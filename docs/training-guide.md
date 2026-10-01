# Training Guide

How to take a Botropolis agent from its spec to its own fine-tuned model.
I built this flow around one idea: the dataset is the product. Get the
examples right and the training part is mostly plumbing.

## 1. Collect the dataset

Format is JSONL, one chat per line, OpenAI style messages:

```json
{"messages": [
  {"role": "system", "content": "You are Ethan, ..."},
  {"role": "user", "content": "..."},
  {"role": "assistant", "content": "..."}
], "eval_keywords": ["..."]}
```

Where the examples come from, in order of quality:

1. **Hand written.** Slow, but the best signal. Write 50 to 100 to start.
2. **Distilled.** Have a strong model answer in the agent's voice, then
   hand review every row. Review is not optional; distillation copies
   mistakes faithfully.
3. **Rewritten logs.** Take real agent runs, fix the weak responses, keep
   the good ones.

Aim for hundreds of rows minimum before a real run. Cover every task type
in the agent's `example_tasks`. Hold out 10 percent for eval and never
train on it.

## 2. Write the config

Copy `training/configs/sft_base.yaml` (full fine-tune) or
`training/configs/lora_scout.yaml` (LoRA, cheaper and usually enough for
teaching style and task format). Point `dataset.path` at your file. The
learning rates in the templates are sane starting points: 2e-5 for full
SFT, 1e-4 for LoRA.

## 3. Validate and plan

```bash
python training/train.py --config training/configs/lora_scout.yaml --dry-run
```

This checks the config and the dataset, prints the plan (steps, batch
size, output dir), and writes `run_plan.json`. Fix any errors it reports
before spending GPU time.

## 4. Train

This repo does not bundle a trainer, so run the plan with your tool of
choice. The marked hook is `launch_trainer()` in `training/train.py`.
What has worked for me:

- **TRL** (`SFTTrainer`) for straightforward SFT and LoRA runs.
- **Unsloth** when VRAM is tight.
- **Axolotl** when I want the config file to be the whole story.

Save adapters or merged weights under `checkpoints/<model-id>/`. That
directory is gitignored; push the weights to Hugging Face or keep the
GGUF locally instead.

## 5. Evaluate

```bash
python training/evaluate.py --dataset training/datasets/<eval>.jsonl --model <model-id>
```

The harness checks expected keywords per row and prints per-example
scores plus the average. For a real eval, also grade 30 to 50 responses
by hand against a rubric: task decomposition, factual honesty, voice
match. Keyword scores catch regressions; humans catch quality.

## 6. Register

1. Write the model card from `models/cards/template.md`.
2. Add the entry to `models/registry.yaml` under `fine_tunes` with
   status `ready` and the weight locations filled in.
3. Point the agent's spec at the new model id.
4. Run the test suite and the demo to confirm the company still works.

## Costs, honestly

A LoRA run on a 7b/8b model with a few thousand rows fits on a single
24GB GPU and finishes in a few hours. Full fine-tunes cost multiples of
that. Start with LoRA; move to full SFT only if the evals say the style
is right but the knowledge is not sticking.
