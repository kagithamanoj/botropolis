# Botropolis

**A company of bots.**

Botropolis is a multi-agent framework where specialized AI agents are
organized like company departments. You ask the CEO for something, the CEO
breaks it into pieces, and the right departments do the work. Twenty agents
across ten departments, one orchestrator, and a training pipeline so each
agent can eventually get its own fine-tuned model.

I built this because most agent frameworks feel like a pile of scripts.
Companies have an org chart for a reason: it makes clear who does what.
This is that idea, in Python.

## Org chart

```
                        +------------------+
                        |       CEO        |
                        |  (orchestrator)  |
                        +--------+---------+
                                 |
        +-----------+------------+------------+------------+
        |           |            |            |            |
    research     health      finance       code         data
   /   |   \    /  |  \       /   \       /  |  \       /   \
 Scout Analyst FactChecker TriageBot WellnessCoach MedResearcher ...
```

Full roster:

| Department | Agents |
|------------|--------|
| research   | Scout (web research), Analyst (synthesis), FactChecker (verification) |
| health     | TriageBot (symptom triage), WellnessCoach (habits), MedResearcher (literature) |
| finance    | MarketAnalyst (markets), BudgetPlanner (budgeting) |
| code       | Coder, Reviewer, DevOps |
| data       | DataEngineer (pipelines), MLEngineer (training and eval) |
| legal      | Paralegal (research summaries) |
| marketing  | Copywriter, SEOAnalyst |
| ops        | Scheduler, InboxAssistant |
| security   | SecAuditor |
| support    | SupportAgent |

Health and legal agents carry explicit disclaimers in their prompts. They
give guidance and summaries, not diagnoses or legal advice.

## Quickstart

Requires Python 3.10+.

```bash
git clone https://github.com/kagithamanoj/botropolis.git
cd botropolis
pip install -e ".[dev]"   # or: pip install pyyaml requests fastapi uvicorn
```

Run the demo. It works with no API keys, agents answer with structured
offline reasoning that is clearly labeled as such:

```bash
python examples/demo.py
```

Ask the CEO anything:

```python
from botropolis.core.orchestrator import CEO

report = CEO().handle("Research vector databases and compare pricing")
print(report.summary)
for result in report.results:
    print(result.agent_name, "->", result.output[:200])
```

Run the API server:

```bash
uvicorn botropolis.server:app --reload
# POST /ask {"request": "..."}   GET /agents   GET /departments
```

## Models

Every agent spec names a model. `models/registry.yaml` is the central
list: base models up top, fine-tunes below. To point an agent at a
different model, change the `model` field in its spec.

API keys come from the environment:

```bash
export OPENAI_API_KEY=...
export ANTHROPIC_API_KEY=...
export GOOGLE_API_KEY=...
```

No keys set? Everything still runs. Agents fall back to an offline stub
that returns structured reasoning, clearly labeled. Good enough for
development and for the test suite.

Local models work through Ollama. Point a spec at an Ollama tag
(e.g. `llama3.3`) and keep `ollama serve` running.

## Training

The `training/` folder is the pipeline for giving agents their own
fine-tuned models:

- `configs/` holds example SFT and LoRA configs
- `datasets/sample.jsonl` shows the expected chat JSONL format
- `train.py` validates a config and dataset, then writes a run plan
  (dry run by default; plug in a real trainer like TRL when you have GPUs)
- `evaluate.py` scores a dataset with simple keyword checks

The full workflow is in `docs/training-guide.md`. When a fine-tune is
done, register it in `models/registry.yaml` under `fine_tunes` and write
a card in `models/cards/`. There is a template plus a filled example.

## Project structure

```
botropolis/
  core/        agent base class, registry, CEO orchestrator, model client
  agents/      one folder per department, one YAML spec per agent
  tools/       shared tools agents can call
  server.py    FastAPI: POST /ask, GET /agents, GET /departments
models/        registry.yaml and model cards
training/      configs, datasets, train.py, evaluate.py
examples/      demo.py, add_agent.py
docs/          architecture, adding agents, adding models, training guide
tests/
```

## Adding an agent

1. Pick a department folder under `botropolis/agents/` (or make a new one)
2. Copy an existing spec and edit it: name, title, specialty, model,
   tools, system prompt, example tasks
3. Run `pytest` to confirm the registry picks it up

Details in `docs/adding-agents.md`.

## Roadmap

- Real tool execution wired into the model clients (function calling)
- Conversation memory per agent
- Department-level fine-tunes registered with eval scores
- A simple web UI on top of the API server
- Nightly eval runs in CI

## License

MIT. See LICENSE.
