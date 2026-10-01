# Adding Models

Models live in `models/registry.yaml`. There are two lists: `base_models`
for off the shelf models, and `fine_tunes` for models trained for
Botropolis agents.

## Adding a base model

Append an entry under `base_models`:

```yaml
- id: my-model-name
  provider: openai        # openai | anthropic | google | ollama
  context_window: 128000
  notes: What it is good at.
```

Provider determines how `ModelClient` calls it and which env var it needs:

| Provider  | Env var           | Notes                              |
|-----------|-------------------|------------------------------------|
| openai    | OPENAI_API_KEY    | Chat completions API               |
| anthropic | ANTHROPIC_API_KEY | Messages API                       |
| google    | GOOGLE_API_KEY    | Gemini generateContent              |
| ollama    | OLLAMA_HOST       | Local server, default localhost:11434 |

No key set means the offline stub answers for that model. That is
intentional: everything runs without keys, and stub output is always
labeled.

## Registering a fine-tuned model

After training (see `docs/training-guide.md`), add an entry under
`fine_tunes`:

```yaml
- id: botropolis-scout-7b
  status: ready                    # planned | training | ready | archived
  base_model: llama-3.1-8b
  trained_for: Ethan (research department)
  dataset: training/datasets/scout-v1.jsonl
  config: training/configs/lora_scout.yaml
  hf_hub: "kagithamanoj/botropolis-scout-7b"   # if pushed to Hugging Face
  local_path: "./checkpoints/botropolis-scout-7b"  # safetensors directory
  gguf_path: "./checkpoints/botropolis-scout-7b-q4_k_m.gguf"  # for llama.cpp
  ollama_tag: "botropolis-scout-7b"             # after `ollama create`
```

Fill in whichever location fields apply. At least one should be set before
you flip status to `ready`.

## Pointing an agent at the model

Change the `model:` field in the agent's spec:

```yaml
# botropolis/agents/research/scout.yaml
model: botropolis-scout-7b
```

The next run resolves the id through the registry. For local weights, the
simplest path is `ollama create botropolis-scout-7b -f Modelfile` from the
GGUF, then the `ollama` provider picks it up with no code changes.

## Model cards

Every fine-tune gets a card in `models/cards/`, copied from
`models/cards/template.md`. The card records intended use, training data,
evals, and limitations. I treat a missing card as a missing model: if it
is not documented, it is not registered.
