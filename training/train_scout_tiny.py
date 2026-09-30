#!/usr/bin/env python3
"""Train botropolis-scout-tiny: a small decoder-only transformer on CPU.

Demo-scale model. Proves the training pipeline end to end: data, tokenizer,
training loop, checkpointing, eval, generation. Not a competitive LLM.

Usage:
    .venv-train/bin/python training/train_scout_tiny.py [--epochs 50]

Outputs (models/weights/botropolis-scout-tiny/):
    model.pt       best checkpoint (state_dict, fp32)
    tokenizer.json word -> id mapping plus specials
    config.json    architecture and training config
    eval.json      val perplexity and sample generations
"""

import argparse
import json
import math
import random
import re
from pathlib import Path

import torch
import torch.nn as nn
import torch.nn.functional as F

ROOT = Path(__file__).resolve().parents[1]
DATA = ROOT / "training" / "datasets" / "scout_demo.jsonl"
OUT_DIR = ROOT / "models" / "weights" / "botropolis-scout-tiny"

PAD, UNK, BOS, EOS = "<pad>", "<unk>", "<bos>", "<eos>"

CONFIG = {
    "d_model": 256,
    "n_layer": 6,
    "n_head": 4,
    "d_ff": 1024,
    "dropout": 0.1,
    "block_size": 128,
    "vocab_cap": 8000,
    "min_freq": 2,
    "lr": 3e-4,
    "batch_size": 16,
    "weight_decay": 0.01,
    "seed": 7,
}


def tokenize_words(text):
    return re.findall(r"\w+|[^\w\s]", text)


def format_example(messages):
    parts = []
    for m in messages:
        role = {"system": "System", "user": "User",
                "assistant": "Assistant"}[m["role"]]
        parts.append(f"{role}: {m['content']}")
    return "\n".join(parts)


def build_vocab(texts):
    from collections import Counter
    counts = Counter()
    for t in texts:
        counts.update(tokenize_words(t))
    vocab = {PAD: 0, UNK: 1, BOS: 2, EOS: 3}
    for word, c in counts.most_common():
        if c < CONFIG["min_freq"] or len(vocab) >= CONFIG["vocab_cap"]:
            break
        if word not in vocab:
            vocab[word] = len(vocab)
    return vocab


def encode(text, vocab):
    ids = [vocab[BOS]]
    ids.extend(vocab.get(w, vocab[UNK]) for w in tokenize_words(text))
    ids.append(vocab[EOS])
    return ids


class Block(nn.Module):
    def __init__(self, d_model, n_head, d_ff, dropout):
        super().__init__()
        self.ln1 = nn.LayerNorm(d_model)
        self.attn = nn.MultiheadAttention(d_model, n_head, dropout=dropout,
                                          batch_first=True)
        self.ln2 = nn.LayerNorm(d_model)
        self.mlp = nn.Sequential(
            nn.Linear(d_model, d_ff), nn.GELU(),
            nn.Linear(d_ff, d_model), nn.Dropout(dropout),
        )

    def forward(self, x, mask):
        a, _ = self.attn(self.ln1(x), self.ln1(x), self.ln1(x),
                         attn_mask=mask, need_weights=False)
        x = x + a
        x = x + self.mlp(self.ln2(x))
        return x


class TinyGPT(nn.Module):
    def __init__(self, vocab_size, d_model, n_layer, n_head, d_ff,
                 dropout, block_size):
        super().__init__()
        self.block_size = block_size
        self.tok_emb = nn.Embedding(vocab_size, d_model)
        self.pos_emb = nn.Embedding(block_size, d_model)
        self.blocks = nn.ModuleList(
            Block(d_model, n_head, d_ff, dropout) for _ in range(n_layer))
        self.ln = nn.LayerNorm(d_model)
        self.head = nn.Linear(d_model, vocab_size, bias=False)

    def forward(self, idx):
        b, t = idx.shape
        x = self.tok_emb(idx) + self.pos_emb(
            torch.arange(t, device=idx.device)).unsqueeze(0)
        mask = torch.triu(torch.ones(t, t, device=idx.device,
                                     dtype=torch.bool), diagonal=1)
        for blk in self.blocks:
            x = blk(x, mask)
        return self.head(self.ln(x))

    def count_params(self):
        return sum(p.numel() for p in self.parameters())


def chunk_tokens(all_ids, block_size):
    # drop the tail so every block is full; x predicts next token
    n = (len(all_ids) - 1) // block_size
    xs, ys = [], []
    for i in range(n):
        s = i * block_size
        xs.append(all_ids[s:s + block_size])
        ys.append(all_ids[s + 1:s + 1 + block_size])
    return (torch.tensor(xs, dtype=torch.long),
            torch.tensor(ys, dtype=torch.long))


@torch.no_grad()
def eval_loss(model, xb, yb, batch_size):
    model.eval()
    total, n = 0.0, 0
    for i in range(0, len(xb), batch_size):
        logits = model(xb[i:i + batch_size])
        loss = F.cross_entropy(
            logits.reshape(-1, logits.size(-1)), yb[i:i + batch_size].reshape(-1))
        total += loss.item() * len(xb[i:i + batch_size])
        n += len(xb[i:i + batch_size])
    return total / n


@torch.no_grad()
def generate(model, vocab, inv_vocab, prompt, max_new=90):
    model.eval()
    ids = encode(prompt, vocab)[:-1]  # drop trailing EOS, we want continuation
    ids = ids[-CONFIG["block_size"]:]
    for _ in range(max_new):
        x = torch.tensor([ids[-CONFIG["block_size"]:]], dtype=torch.long)
        logits = model(x)[0, -1]
        nxt = int(torch.argmax(logits))
        ids.append(nxt)
        if nxt == vocab[EOS]:
            break
    words = [inv_vocab[i] for i in ids
             if inv_vocab[i] not in (BOS, EOS, PAD)]
    out = " ".join(words)
    out = re.sub(r"\s+([.,;:!?%)\]])", r"\1", out)
    out = re.sub(r"([(\[{])\s+", r"\1", out)
    out = re.sub(r"(\w)-\s+(\w)", r"\1-\2", out)
    out = re.sub(r"\s+", " ", out).strip()
    return out


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--epochs", type=int, default=50)
    args = ap.parse_args()

    random.seed(CONFIG["seed"])
    torch.manual_seed(CONFIG["seed"])
    torch.set_num_threads(2)

    rows = [json.loads(l) for l in open(DATA)]
    texts = [format_example(r["messages"]) for r in rows]
    random.shuffle(texts)
    n_val = max(1, len(texts) // 10)
    val_texts, train_texts = texts[:n_val], texts[n_val:]
    print(f"train examples: {len(train_texts)}, val examples: {len(val_texts)}",
          flush=True)

    vocab = build_vocab(texts)
    inv_vocab = {i: w for w, i in vocab.items()}
    print(f"vocab size: {len(vocab)}", flush=True)

    train_ids = [t for txt in train_texts for t in encode(txt, vocab)]
    val_ids = [t for txt in val_texts for t in encode(txt, vocab)]
    xtr, ytr = chunk_tokens(train_ids, CONFIG["block_size"])
    xva, yva = chunk_tokens(val_ids, CONFIG["block_size"])
    print(f"train tokens: {len(train_ids)}, val tokens: {len(val_ids)}, "
          f"train blocks: {len(xtr)}, val blocks: {len(xva)}", flush=True)

    model = TinyGPT(len(vocab), CONFIG["d_model"], CONFIG["n_layer"],
                    CONFIG["n_head"], CONFIG["d_ff"], CONFIG["dropout"],
                    CONFIG["block_size"])
    print(f"params: {model.count_params() / 1e6:.2f}M", flush=True)
    assert model.count_params() <= 15_000_000

    opt = torch.optim.AdamW(model.parameters(), lr=CONFIG["lr"],
                            weight_decay=CONFIG["weight_decay"])
    bs = CONFIG["batch_size"]
    best_val, best_state = float("inf"), None

    for epoch in range(1, args.epochs + 1):
        model.train()
        perm = torch.randperm(len(xtr))
        tot, n = 0.0, 0
        for i in range(0, len(xtr), bs):
            idx = perm[i:i + bs]
            logits = model(xtr[idx])
            loss = F.cross_entropy(logits.reshape(-1, logits.size(-1)),
                                   ytr[idx].reshape(-1))
            opt.zero_grad()
            loss.backward()
            torch.nn.utils.clip_grad_norm_(model.parameters(), 1.0)
            opt.step()
            tot += loss.item() * len(idx)
            n += len(idx)
        vl = eval_loss(model, xva, yva, bs)
        print(f"epoch {epoch:3d} train_loss {tot / n:.4f} "
              f"val_loss {vl:.4f} val_ppl {math.exp(vl):.2f}", flush=True)
        if vl < best_val:
            best_val = vl
            best_state = {k: v.cpu().clone()
                          for k, v in model.state_dict().items()}

    OUT_DIR.mkdir(parents=True, exist_ok=True)
    torch.save(best_state, OUT_DIR / "model.pt")
    with open(OUT_DIR / "tokenizer.json", "w") as fh:
        json.dump({"vocab": vocab,
                   "specials": {"pad": PAD, "unk": UNK, "bos": BOS,
                                "eos": EOS}}, fh)
    with open(OUT_DIR / "config.json", "w") as fh:
        json.dump({**CONFIG, "vocab_size": len(vocab),
                   "params": model.count_params(),
                   "epochs": args.epochs,
                   "best_val_loss": best_val,
                   "best_val_ppl": math.exp(best_val)}, fh, indent=2)

    # generations from the best checkpoint
    model.load_state_dict(best_state)
    prompts = [
        "System: You are Scout, the web research specialist at Botropolis.\n"
        "User: Give me a quick brief on pgvector.\nAssistant:",
        "System: You are Scout, the web research specialist at Botropolis.\n"
        "User: What are the key trade-offs with LoRA and QLoRA?\nAssistant:",
        "System: You are Scout, the web research specialist at Botropolis.\n"
        "User: Can you research prompt caching for me?\nAssistant:",
    ]
    samples = []
    for p in prompts:
        gen = generate(model, vocab, inv_vocab, p)
        samples.append({"prompt": p.split("User: ")[1], "generation": gen})
        print("---", flush=True)
        print(gen, flush=True)
    with open(OUT_DIR / "eval.json", "w") as fh:
        json.dump({"val_perplexity": math.exp(best_val),
                   "val_loss": best_val,
                   "note": "Val set shares answer templates with train; "
                           "ppl reflects format learning, not general quality.",
                   "samples": samples}, fh, indent=2)
    print(f"\nval_ppl {math.exp(best_val):.2f} | saved to {OUT_DIR}", flush=True)


if __name__ == "__main__":
    main()
