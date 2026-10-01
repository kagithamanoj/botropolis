# Architecture

Botropolis is organized like a company because that turned out to be the
clearest way to think about multi-agent systems. Here is how the metaphor
maps to the code.

## The org chart

```
                        +------------------+
                        |       CEO        |
                        |  (orchestrator)  |
                        +--------+---------+
                                 |
        +-----------+------------+------------+-----------+
        |           |            |            |           |
   research      health      finance       code  ... (10 total)
   /  |  \      /  |  \
 Ethan Ava Jack ...
```

## Manoj, the CEO (orchestrator)

`botropolis/core/orchestrator.py` holds the `CEO` class. It does five things:

1. **Plan.** Scores the request against keyword lists per department and
   picks up to three relevant departments. No keywords hit, research gets
   the job by default.
2. **Assign.** Picks the lead agent for each chosen department
   (`LEAD_AGENT` map), or the first agent alphabetically as a fallback.
3. **Report.** Runs each agent, collects `AgentResult` objects, and builds
   a `CompanyReport` with an executive summary.
4. **Team.** `team()` runs named agents in collaboration rounds: each
   agent's prompt includes prior teammates' outputs, and Manoj synthesizes
   the rounds into one answer.
5. **Remember.** `handle()` and `plan()` take an optional chat history.
   The tail of it is attached to every task as context, and when the
   request itself names no department, routing falls back to scoring the
   request plus recent history. Current words always win.

The CEO never does the work itself. That is the whole point.

## Departments and agents

`botropolis/agents/<department>/` holds one YAML spec per agent. The spec
is the agent's identity: name, title, department, specialty, model, tools,
system prompt, example tasks. `botropolis/core/agent.py` loads the spec
and gives the agent a `run(task)` method.

`botropolis/core/registry.py` is the company directory. It discovers every
`agents/*/*.yaml` at startup and answers `get(name)`,
`by_department(dept)`, and `list_all()`.

## Models

`botropolis/core/models.py` holds `ModelClient`. It resolves which
provider serves a model id using `models/registry.yaml`, then calls that
provider's HTTP API directly with `requests`. No SDK dependencies.

Credentials come from the environment:

- `OPENAI_API_KEY`
- `ANTHROPIC_API_KEY`
- `GOOGLE_API_KEY`
- `OLLAMA_HOST` (defaults to `http://localhost:11434`)

No key for the requested model means the deterministic offline stub
answers instead. The stub is labeled clearly in every response, so you
always know when you are looking at stub output versus a real model call.

## Tools

`botropolis/tools/builtin.py` defines the `Tool` base class and ten
builtin tools: `web_search`, `web_fetch`, `shell`, `calculator`,
`read_file`, `write_file`, `list_dir`, `current_time`, `notes_append`,
`notes_read`. Agents declare which tools they may use in their spec.
`web_search` is stub-backed until you wire a real provider; the schema is
stable so swapping the implementation is a one function change.

`botropolis/tools/gws.py` adds five Google Workspace tools:
`gmail_search`, `gmail_read`, `gmail_draft`, `calendar_agenda`,
`calendar_create_event`. They are registered alongside the builtins, so
agents name them in `toolkit` like any other tool. Deliberate limits: no
send tool (agents draft, humans send), and calendar events are private
only. When the Workspace CLI is not connected, the tools report that
plainly instead of failing.

## Schemas

`botropolis/core/schemas.py` has the six dataclasses everything passes
around: `Task` (work to do), `ToolCall` (one tool invocation),
`AgentResult` (what one agent produced), `CompanyReport` (the CEO's final
answer), plus `TeamRound` and `TeamReport` for collaboration runs. All
serialize with `to_dict()`, which is what the API server returns.

## Server

`botropolis/server.py` is a thin FastAPI layer over the CEO. The web
layer does no reasoning of its own.

- `POST /ask` takes a request string plus an optional chat history and
  returns a `CompanyReport` as JSON.
- `POST /team` runs named agents in collaboration rounds and returns a
  `TeamReport`.
- `GET /agents/{name}/ask/stream` streams one agent's run as
  server-sent events: `tool_started`, `tool_finished`, then `result`.
- `GET /analytics` returns per-agent usage totals: requests,
  tool calls, which tools each agent used, tokens, latency, errors.

Every request is also appended to a local JSONL usage log
(`botropolis/data/usage.jsonl`, gitignored) with a tool-call count, so
the analytics tab works without any external service.

## Training

`training/` is separate from the runtime on purpose. Datasets, configs,
the plan-only trainer runner, and the eval harness live there. A trained
model re-enters the system by being registered in `models/registry.yaml`
and referenced from an agent spec. See `docs/training-guide.md`.
