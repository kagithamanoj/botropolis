# Training

This is where Botropolis agents get their own fine-tuned models. The flow is
simple: collect examples of the agent doing good work, train a small model on
them, evaluate it, and register it in `models/registry.yaml`.

## Two paths

**Path 1: CPU demo, fully in-repo.** `training/train_scout_tiny.py` trains a
~6M parameter decoder-only transformer from scratch on CPU, no GPU needed.
It builds a word-level tokenizer from the data, trains next-token prediction
on `training/datasets/scout_demo.jsonl` (300 synthetic Scout Q&A pairs made
by `training/make_scout_demo_dataset.py`), and saves the best checkpoint to
`models/weights/botropolis-scout-tiny/` with tokenizer, config, and eval
results. This proves the pipeline end to end. The model is a demo, not
something to deploy.

**Path 2: real LoRA fine-tune on a free GPU.** `training/colab_lora.py` is a
ready-to-run script for Colab or Kaggle: it LoRA-tunes SmolLM2-135M (or
Qwen2.5-0.5B) on any dataset in the repo's chat JSONL format and saves the
adapter. Setup steps are commented at the top of the file. This is how you
get a model worth registering and actually using.

## How it works here

This repo does not bundle a GPU trainer. `train.py` validates your config and
dataset, prints a concrete training plan, and writes `run_plan.json` into the
output dir. That plan is what you hand to a real trainer (Hugging Face TRL,
Axolotl, Unsloth, or a cloud job). The code is structured so the trainer
call is one clearly marked function to swap in.

`evaluate.py` is a small harness that runs a dataset through a model (or the
offline stub) and checks expected keywords, printing an accuracy style
report. It runs anywhere, no GPU needed.

## Dataset format

JSONL, one example per line. Each example is a chat in OpenAI format, plus
optional eval fields:

```json
{"messages": [
  {"role": "system", "content": "You are Scout, the web research specialist..."},
  {"role": "user", "content": "What are small language models good for?"},
  {"role": "assistant", "content": "Small language models are useful for..."}
], "eval_keywords": ["edge", "quantization"]}
```

Rules I follow when building datasets:
- Every assistant message should sound like the agent's system prompt.
- Cover the task types in the agent's `example_tasks` from its spec.
- Keep a held out eval split that never appears in training.
- 6 rows in `training/datasets/sample.jsonl` show the shape. Real runs want hundreds to thousands.

## Configs

- `configs/sft_base.yaml`: plain supervised fine-tuning, the starting template.
- `configs/lora_scout.yaml`: LoRA fine-tune aimed at the Scout research agent.

## Steps to train and register

1. Collect data into `training/datasets/<name>.jsonl` in the format above.
2. Copy a config, point it at your dataset, tune epochs and learning rate.
3. Run `python training/train.py --config training/configs/<yours>.yaml` to validate and get the plan.
4. Train with your trainer of choice using the printed plan.
5. Run `python training/evaluate.py --dataset training/datasets/<eval>.jsonl` to score it.
6. Write a model card from `models/cards/template.md`.
7. Register the model in `models/registry.yaml` under `fine_tunes` and flip status to `ready`.
8. Point the agent's spec at the new model id.

See `docs/training-guide.md` for the full walkthrough.
