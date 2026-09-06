# Model Card: botropolis-scout-7b

## Model details
- Base model: llama-3.1-8b
- Fine-tune method: LoRA (rank 16, alpha 32), supervised fine-tuning
- Trained for agent: Scout (research department, web research specialist)
- Version / date: v0.1 planned, not yet trained
- License: Intended for release under the base model's license terms

## Intended use
A small, cheap research assistant that decomposes a question into searchable
sub-questions, summarizes findings, and separates confirmed facts from
inference. Built to run locally via Ollama so research stays private and
free. Not intended for medical, legal, or financial advice.

## Training data
Planned: a few thousand instruction pairs in chat format, written in Scout's
voice. Topics cover question decomposition, multi-angle evidence gathering,
and date-aware summarization. Example rows live in
training/datasets/sample.jsonl. No user data, no scraped personal data.

## Evaluation
Planned: the keyword harness in training/evaluate.py plus a held out set of
50 research questions graded by hand for decomposition quality and citation
honesty. Target is to beat the base model on decomposition quality before
any release.

## Limitations
Small model, so it will be worse than frontier models at nuanced synthesis.
It can still hallucinate sources if pressed, so the "never invent sources"
behavior needs explicit eval coverage. English only for now.

## Ethical considerations
Research assistants can amplify misinformation if they sound confident about
weak evidence. The training data rewards hedging language and explicit
confidence labels. Deployment should keep the FactChecker agent in the loop
for high stakes claims.

## How to run
Registered in models/registry.yaml as `botropolis-scout-7b` (status: planned).
Once trained, planned Ollama tag: `botropolis-scout-7b`.
