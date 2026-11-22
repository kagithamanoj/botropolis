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
 Scout Analyst FactChecker ...
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

`botropolis/tools/builtin.py` defines the `Tool` base class and eleven
builtin tools: `web_search`, `web_fetch`, `shell`, `calculator`,
`read_file`, `write_file`, `list_dir`, `current_time`, `notes_append`,
`notes_read`, and `math`. Agents declare which tools they may use in
their spec. `web_search` is stub-backed until you wire a real provider;
the schema is stable so swapping the implementation is a one function
change.

The `math` tool (`botropolis/tools/mathtool.py`) does symbolic math
with sympy: solve, simplify, factor, expand, derivatives, integrals,
limits, series, and numeric evaluation. Parsing is defensive: the raw
string passes a character whitelist and an identifier whitelist
before sympy sees it, and the parser namespace holds only the allowed
symbols and functions. Nothing evals Python.

`botropolis/tools/gws.py` adds five Google Workspace tools:
`gmail_search`, `gmail_read`, `gmail_draft`, `calendar_agenda`,
`calendar_create_event`. They are registered alongside the builtins, so
agents name them in `toolkit` like any other tool. Deliberate limits: no
send tool (agents draft, humans send), and calendar events are private
only. When the Workspace CLI is not connected, the tools report that
plainly instead of failing.

`botropolis/tools/jobs.py` adds two job hunting tools. `job_search`
reads the RemoteOK public feed and returns postings matched by keyword
against title, company, and tags; a feed that cannot be fetched or
parsed is an error, never a silent empty result. `job_apply` prepares
an application without submitting anything: it checks the resume lives
in the workspace, records the application in
`botropolis/data/applications.jsonl`, and returns the apply url so a
human or a browser flow does the actual submission. Recording the same
title and company twice returns a warning instead of a duplicate.

## Schemas

`botropolis/core/schemas.py` has the dataclasses everything passes
around: `Task` (work to do), `Context` (what the agent may know beyond
the task: chat history, teammates' outputs), `Contract` (the agreement
for a run, with its machine checks), `ToolCall` (one tool
invocation), `AgentResult` (what one agent produced, including the
validated `structured` answer), `CompanyReport` (the CEO's final
answer), plus `TeamRound` and `TeamReport` for collaboration runs. All
serialize with `to_dict()`, which is what the API server returns.

## Output contracts

Every agent declares an `output_schema` in its spec (or takes the
department default) from `botropolis/core/output_schemas.py`. The
framework asks the model for JSON matching the schema, validates it
with Pydantic, and retries once with the errors spelled out. A
twice-failed answer becomes a failed result, never silent garbage.

## Run ledger

`botropolis/core/ledger.py` is an append-only, SQLite-backed event log.
Every run appends `run_started`, `task_assigned`, `tool_called`,
`output_produced`, `output_repair`, `agent_failed`, and `run_finished`
events under one run id. The CEO keeps an in-memory ledger by default;
pass `Ledger("checkpoints/runs.db")` to persist the trail. Recording
never breaks a run: a failing ledger is swallowed, not propagated.

## Harness

The model is one part of a run. The harness around it is the rest:
the contract the work is held to, the walls it cannot cross, the
state it leaves behind, and the checks that decide whether "done"
means done. Five pieces:

**Contracts.** A `Contract` (goal, scope, exclusions, done_when, and
optional machine checks) can be attached to a run through
`CEO.handle()` and `CEO.team()`, or in the `POST /ask` body. The CEO
puts it on every task, each agent sees it in its prompt stated
plainly (exclusions as out of scope), the ledger notes it at run
start, and the final report carries it back.

**Loop exits.** The tool loop stops for more than `max_steps` now.
The same tool call failing with the same error twice in a row ends
the run: retrying an identical failure buys nothing. Agents can also
carry a token budget and a time budget, in the spec or per run, and
going over either stops the run. Both exits produce a failed result
marked escalated with the reason, recorded as `run_escalated`.

**Run state and resume.** Tool loops write their progress after
every step (task, contract, completed tool calls with results,
status) to `botropolis/data/runs/<run_id>.json`
(`botropolis/core/runstate.py`, gitignored). `botropolis resume
<run_id>` rebuilds the conversation from the completed steps and
continues from the first unfinished one; finished tool calls are
shown to the model as observations, not re-executed. A run that
already finished says so and exits.

**Done evidence.** A contract's machine checks are shell commands
with an expected exit code (`botropolis/core/evidence.py`). Before
the CEO finalizes a report for a contracted run, the checks run
inside the agent sandbox and the results ride on the report as
evidence. All passing keeps the report `done`; any failure makes it
`unverified`. The agent claims, the harness verifies.

**Approval wall.** A run can flag tools as needing approval
(`needs_approval` on the task, `approved` for the human's yes). At
dispatch, a flagged call on an unapproved run never executes: the
model gets a blocked observation, the call is recorded on the
result, and the ledger gets a `tool_blocked` event. Nothing is
flagged by default.

## Server

`botropolis/server.py` is a thin FastAPI layer over the CEO. The web
layer does no reasoning of its own.

- `POST /ask` takes a request string plus an optional chat history
  and returns a `CompanyReport` as JSON. The body also accepts an
  optional contract and the approval wall fields (`needs_approval`,
  `approved`); see the Harness section.
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
