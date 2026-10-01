#!/usr/bin/env python3
"""LoRA fine-tune for a Botropolis agent on a free Colab / Kaggle GPU.

This is the path to a SERIOUS model: take a small open base model
(SmolLM2-135M by default) and LoRA-tune it on the repo's chat JSONL
datasets. The CPU demo (train_scout_tiny.py) proves the pipeline;
this script trains something you can actually use.

SETUP (run once per Colab session):
    # 1. GPU runtime: Runtime -> Change runtime type -> T4 GPU
    # 2. Install deps (takes a few minutes):
    !pip install -q transformers peft trl datasets accelerate bitsandbytes
    # 3. Upload your dataset: drag training/datasets/scout_demo.jsonl
    #    (or your own) into the Colab file panel, note the path below.
    # 4. Run this script:
    !python colab_lora.py --dataset /content/scout_demo.jsonl \
        --base HuggingFaceTB/SmolLM2-135M --out /content/scout-lora

EXPECTED RUNTIME: ~20-40 min on a T4 for 300 examples x 3 epochs.
WHAT YOU GET: a LoRA adapter in --out, plus tokenizer. Merge or load
with peft, or push to the Hub and register it in models/registry.yaml.

Kaggle alternative: same script works; enable GPU in notebook settings,
internet on for the base model download.
"""

import argparse
import json
from pathlib import Path

import torch
from datasets import Dataset
from transformers import (AutoModelForCausalLM, AutoTokenizer,
                          TrainingArguments)
from peft import LoraConfig, get_peft_model, prepare_model_for_kbit_training
from trl import SFTTrainer


def load_chat_jsonl(path):
    rows = [json.loads(l) for l in open(path)]
    texts = []
    for r in rows:
        parts = []
        for m in r["messages"]:
            role = {"system": "System", "user": "User",
                    "assistant": "Assistant"}[m["role"]]
            parts.append(f"{role}: {m['content']}")
        texts.append("\n".join(parts))
    return Dataset.from_dict({"text": texts})


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--dataset", required=True,
                    help="chat JSONL in the repo format")
    ap.add_argument("--base", default="HuggingFaceTB/SmolLM2-135M",
                    help="base model id (try Qwen/Qwen2.5-0.5B for bigger)")
    ap.add_argument("--out", default="./scout-lora",
                    help="where to save the adapter")
    ap.add_argument("--epochs", type=int, default=3)
    ap.add_argument("--lr", type=float, default=2e-4)
    ap.add_argument("--rank", type=int, default=16,
                    help="LoRA rank; 8-32 is the usual range")
    args = ap.parse_args()

    ds = load_chat_jsonl(args.dataset)
    print(f"loaded {len(ds)} examples from {args.dataset}", flush=True)

    tok = AutoTokenizer.from_pretrained(args.base)
    if tok.pad_token is None:
        tok.pad_token = tok.eos_token

    model = AutoModelForCausalLM.from_pretrained(
        args.base,
        load_in_4bit=True,  # QLoRA: fits a T4 easily
        torch_dtype=torch.float16,
        device_map="auto",
    )
    model = prepare_model_for_kbit_training(model)
    peft_cfg = LoraConfig(
        r=args.rank,
        lora_alpha=args.rank * 2,
        lora_dropout=0.05,
        bias="none",
        task_type="CAUSAL_LM",
        target_modules=["q_proj", "k_proj", "v_proj", "o_proj",
                        "gate_proj", "up_proj", "down_proj"],
    )
    model = get_peft_model(model, peft_cfg)
    model.print_trainable_parameters()

    targs = TrainingArguments(
        output_dir=args.out,
        num_train_epochs=args.epochs,
        per_device_train_batch_size=4,
        gradient_accumulation_steps=4,
        learning_rate=args.lr,
        lr_scheduler_type="cosine",
        warmup_ratio=0.05,
        logging_steps=10,
        save_steps=100,
        save_total_limit=2,
        fp16=True,
        optim="paged_adamw_8bit",
        report_to="none",
    )
    trainer = SFTTrainer(
        model=model,
        args=targs,
        train_dataset=ds,
        max_seq_length=1024,
    )
    trainer.train()

    Path(args.out).mkdir(parents=True, exist_ok=True)
    trainer.model.save_pretrained(args.out)
    tok.save_pretrained(args.out)
    print(f"adapter saved to {args.out}", flush=True)
    print("Next: zip it, download it, and register it in "
          "models/registry.yaml as a fine_tune (see the scout-tiny entry).",
          flush=True)


if __name__ == "__main__":
    main()
