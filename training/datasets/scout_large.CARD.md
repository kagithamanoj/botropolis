# Dataset card: scout_large.jsonl

## Basics

- **File:** `training/datasets/scout_large.jsonl`
- **Rows:** 2,940 (one JSON object per line)
- **Schema:** same as `sample.jsonl`: `messages` (system/user/assistant),
  `eval_keywords`, plus a `type` field naming the example type.
- **Source topics:** 210, from `training/datasets/_scout_facts.json`
  (50 CS fundamentals, 20 databases, 15 networking, 15 security, 25 ML/AI,
  15 math/stats, 10 misc tech, 25 science, 20 history, 15 geography).
  Each topic carries 3-6 curated facts: 840 facts total.

## Composition by type

| Type | Rows | What it teaches |
|------|------|-----------------|
| explain | 840 | Plain explanations of a concept from verified facts |
| fact_check | 630 | Verdict on a claim, plus supporting context |
| research_plan | 420 | Breaking a topic into sub-questions and angles |
| summarize | 630 | Key points plus what is still open or debated |
| compare | 420 | Two topics side by side, trade-offs without invented verdicts |

14 examples per topic. All 2,940 prompts are unique; no two examples share
an opening sentence.

## How it was generated

`training/make_scout_large_dataset.py` (seed 20260930) turns each topic's
facts into examples using per-type prompt and response templates with heavy
phrasing rotation. Assistant responses are composed from the curated facts
verbatim, in Scout's voice: confirmed facts stated plainly, inference kept
separate, date-sensitivity noted where it applies, no invented sources or
statistics. `training/validate_dataset.py` checks schema, non-empty fields,
prompt uniqueness, and prints the stats above.

## Intended use

Supervised fine-tuning of the Scout agent via `training/colab_lora.py`
(LoRA on a small open base model, free Colab/Kaggle GPU). The dataset is
sized for that: just under 3k rows, 3 epochs on a T4 is roughly an hour.

## Limitations

- **Synthetic and templated.** Phrasing varies, but the underlying patterns
  repeat. It teaches Scout's reasoning shape and voice more than new facts.
- **Not a substitute for retrieval.** The facts are stable textbook material
  through 2026. Anything time-sensitive still needs live search at inference
  time; the dataset trains the habit of noting dates, not the dates themselves.
- **No adversarial or edge-case coverage.** Prompts are cooperative and
  well-formed. Add red-team rows before trusting the model with hostile input.
- **Scope is introductory.** Each topic gets survey-level coverage. Depth on
  any single topic would need a dedicated dataset.
