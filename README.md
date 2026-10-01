# Botropolis

**A company of bots.**

Botropolis is a multi-agent framework where specialized AI agents are
organized like company departments. You ask Manoj, the CEO, for something,
he breaks it into pieces, and the right departments do the work. Twenty agents
across ten departments, one orchestrator, and a training pipeline so each
agent can eventually get its own fine-tuned model.

I built this because most agent frameworks feel like a pile of scripts.
Companies have an org chart for a reason: it makes clear who does what.
This is that idea, in Python.

## Org chart

```
                        +------------------+
                        |      Manoj       |
                        |  (CEO, orchestrator)|
                        +--------+---------+
                                 |
        +-----------+------------+------------+------------+
        |           |            |            |            |
    research     health      finance       code         data
   /   |   \    /  |  \       /   \       /  |  \       /   \
 Ethan Ava Jack Nina Owen Lena Mason Mia Liam Sofia Noah ...
```

Full roster:

| Department | Agents |
|------------|--------|
| research   | Ethan (web research), Ava (synthesis), Jack (verification) |
| health     | Nina (symptom triage), Owen (habits), Lena (literature) |
| finance    | Mia (markets), Mason (budgeting) |
| code       | Liam, Sofia, Noah |
| data       | Lucas (pipelines), Elena (training and eval) |
| legal      | Henry (research summaries) |
| marketing  | Zara, Lila |
| ops        | Tyler, Anya |
| security   | Ryan |
| support    | Tara |

Health and legal agents carry explicit disclaimers in their prompts. They
give guidance and summaries, not diagnoses or legal advice.

## Architecture diagrams

`docs/architecture.drawio` is a 4-page draw.io set you can open and edit in
[draw.io](https://app.diagrams.net). Each page numbers every step and explains
what happens at each point:

1. System overview: entry points, the FastAPI server, CEO Manoj, all 10
   departments and 20 agents, model providers, and the side systems (model
   registry, training pipeline, usage analytics, tool sandbox).
2. Request lifecycle: what happens step by step when `POST /ask` arrives,
   including the ReAct think-act-observe loop and its safety rails.
3. Teaming: how `POST /team` runs several agents in collaboration rounds.
4. Training pipeline: from dataset to trained model to registry to agent.

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

Ask Manoj anything:

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
# POST /ask {"request": "..."}   POST /agents/{name}/ask   GET /agents   GET /departments   GET /analytics
```

Or use the command line. `pip install -e .` provides the `botropolis`
command:

```bash
botropolis roster --department ops
botropolis ask Liam "summarize this quarter's roadmap"
botropolis team "plan the launch" Liam Sofia --rounds 2
botropolis evals
botropolis serve --port 8000
```

## Web UI

The same server also serves a web UI. No build step, no frameworks,
plain HTML/CSS/JS. Works fine on a phone.

```bash
uvicorn botropolis.server:app --reload
# open http://localhost:8000
```

Five tabs. Chat talks to Manoj: you ask, he routes to departments, and
you get the summary plus one card per agent that did work. Chat keeps
the last few turns as context, so follow-up questions work; Clear wipes
it. Teaming lets
you pick any agents, set the rounds, and watch them collaborate on a task.
War room lets
you pick one agent and talk to it directly, skipping Manoj.
Analytics shows per-agent usage: calls, average latency, and tokens used.
Outputs from the offline stub are labeled as such, and stub runs report
0 tokens. Roster shows every department and
agent with their specs, tools, and example tasks.

Every agent invocation, through Manoj or the war room, is appended to
`botropolis/data/usage.jsonl` (gitignored). `GET /analytics` returns the
per-agent totals from that log.

## Teaming

One agent working alone is fine. A team is better. `POST /team` runs named
agents in collaboration rounds: each agent's prompt includes the original
request plus everything its teammates produced before it, so the team can
draft, critique, and revise. Manoj then writes a short synthesis of the
final state.

```bash
curl -X POST http://localhost:8000/team \
  -H "Content-Type: application/json" \
  -d '{"request": "Write a Python retry helper", "agents": ["Liam", "Sofia"], "rounds": 2}'
```

Rounds are capped at 3. Unknown agent names return 404. Team runs are
logged to the usage log like any other invocation, so they show up in
`GET /analytics` too.

## Agent tools

Agents can do more than answer from memory. Each agent spec has a
`toolkit` list naming the tools that agent may actually call:

```yaml
toolkit:
  - web_search
  - web_fetch
```

Current toolkits: Ethan and Jack get web search and fetch; Liam
gets shell plus file tools; Sofia gets file reading; Lucas and
Elena get shell, file tools, and the calculator; Anya gets Gmail
search, read, and draft; Tyler gets calendar agenda and event
creation. Every other agent
has no toolkit and behaves exactly as before: one model call, one answer.

When an agent has a toolkit, it runs a think-act-observe loop instead of
a single shot. The protocol is plain text, so it works with any model,
including the offline stub and small fine-tunes. No provider-specific
function calling is involved:

```
ACTION: {"tool": "<name>", "args": {...}}
FINAL: <the answer>
```

The model thinks, emits an ACTION, gets back an `OBSERVATION:` with the
tool result, and repeats until it replies with FINAL. Anything that is
not an ACTION line counts as the final answer, which keeps weak models
and the stub safe by default. Loops stop after `max_steps` (default 8,
configurable per agent in YAML). Every tool call is recorded on the
result and shown in the web UI agent cards.

Available tools: `web_search` (DuckDuckGo, no key needed), `web_fetch`
(page text extraction), `shell`, `read_file`, `write_file`, `list_dir`,
`calculator`, `current_time`, `gmail_search`, `gmail_read`, `gmail_draft`,
`calendar_agenda`, `calendar_create_event`, `notes_append`, `notes_read`.

`notes_append` and `notes_read` share one company notebook
(`botropolis/data/workspace/notes.md`, gitignored): agents can remember
durable facts across runs, and humans can read the same file.

Google Workspace tools shell out to `hatch_gws_cli` and degrade
gracefully when Gmail or Calendar is not connected. Two deliberate
limits: there is no `gmail_send` (agents draft, humans send), and
`calendar_create_event` creates private events only, never inviting
attendees.

Safety model: file and shell tools are confined to an agent workspace
(`botropolis/data/workspace`, or `BOTROPOLIS_WORKSPACE` to move it).
Paths that escape the workspace are refused. Shell commands run with a
30 second timeout and a denylist blocks destructive patterns (`rm -rf /`,
disk writes, fork bombs, and friends). The denylist is a guardrail
against accidents, not a security boundary: treat tool access like
giving a junior engineer a terminal on a scratch machine.

### Watching tool calls live

`GET /agents/{name}/ask/stream?request=...` streams one agent's run as
server-sent events: `tool_started` and `tool_finished` as each tool runs,
then `result` with the full AgentResult, then `done`. The war room tab
uses it to show tool calls live while the agent works.

```bash
curl -N "http://localhost:8000/agents/Liam/ask/stream?request=List%20the%20workspace%20files"
```

## Evals

`botropolis/eval/` holds scenario evals: scripted model conversations
that check what an agent does with them. Each scenario is a YAML file
with the agent's toolkit, the task, the canned model replies, and the
expected output and tool calls:

```bash
python -m botropolis.eval.runner
```

```
[PASS] calculator_two_step
[PASS] plain_answer_no_tools
[PASS] unknown_tool_is_refused

3/3 scenarios passed
```

Scenarios run with no model credentials and no network: the runner
plays the replies through a real Agent and checks the outcome. Add a
new file under `botropolis/eval/scenarios/` to cover a behavior you
care about. A failing scenario exits non-zero with the mismatch
spelled out.

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
  core/        agent base class, registry, Manoj (CEO orchestrator), model client
  agents/      one folder per department, one YAML spec per agent
  tools/       shared tools agents can call
  server.py    FastAPI: POST /ask, POST /agents/{name}/ask, POST /team, GET /agents, GET /departments, GET /analytics
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
- Nightly eval runs in CI

Done:
- A simple web UI on top of the API server

## License

MIT. See LICENSE.
