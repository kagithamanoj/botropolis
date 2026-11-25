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
   The last 40 turns are attached to every task as context, under a
   48000 character budget that drops the oldest turns first, and when the
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

## How the agents work

Four things make an agent good here, and none of them is the raw
model. The know-how lives in the specs: every system prompt is
written like a practitioner in that field, with a working method,
the failure modes to watch for, a quality bar for the output, and a
clear line on when to hand a problem to someone else. The working
habits live in the tool loop: a run may open with a recorded plan,
and a final answer is checked once against the task before it is
accepted (see Harness). When a task turns on another field, an agent
does not guess: the `consult` tool runs the question past the
specialist whose field it is. And what the company learns is kept:
the memory tools save facts with the agent that learned them, and
the CEO attaches the relevant ones to new tasks, so the next run
starts from what the company already knows instead of from zero.

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

`botropolis/tools/memory.py` adds `memory_save` and `memory_recall`,
the tools over the long-term memory store (see Memory below). Every
agent that already has a toolkit gets both; agents without a toolkit
stay single-shot. Saving records which agent learned the fact: the
tool loop fills the name in at dispatch, so the model never supplies
it and cannot spoof it.

`botropolis/tools/consult.py` adds `consult`: an agent names a
specialist and asks one question, and that agent runs the question
as its own task and answers back. The guards are enforced in the
tool, not the prompt: no consulting yourself, a consulted run cannot
consult further (depth one), and each run gets three consults,
counted in a run context the loop creates per run and hands to the
tool at dispatch. An unknown name returns an error the model can
recover from. The registry is resolved at call time, so the server,
the CLI, and tests each pin their own.

## Schemas

`botropolis/core/schemas.py` has the dataclasses everything passes
around: `Task` (work to do), `Context` (what the agent may know beyond
the task: chat history, recalled memories, teammates' outputs), `Contract` (the agreement
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

## Memory

`botropolis/core/memory.py` is the company's long-term memory: a
SQLite-backed store of individual facts (a decision, a name, a number,
a preference) that outlive any single run. It is a different layer
from the notebook: the notebook is one shared page of prose, memory is
searchable rows. Each memory records its text, the agent that saved
it, an importance from 1 to 5, tags, and access stats.

Recall scores by keyword overlap between the query and the fact (text
plus tags), weighted by importance, newest first on ties, and bumps
the access count of whatever it returns. The store lives at
`botropolis/data/memory.db` (gitignored, `BOTROPOLIS_MEMORY_DB` moves
it); `MemoryStore()` with no path is in-memory for tests.

Two paths use it. Agents with a toolkit get `memory_save` and
`memory_recall` and are told to save facts as they learn them and to
recall before answering anything about past work. And when a store is
attached to the CEO, `handle()` recalls the top five facts matching
the request (packed under 2000 chars) onto every task's context, so
even single-shot agents benefit. No store attached, nothing changes.

## Harness

The model is one part of a run. The harness around it is the rest:
the contract the work is held to, the walls it cannot cross, the
state it leaves behind, and the checks that decide whether "done"
means done. Eight pieces:

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

**Plan first, verify before done.** The loop also builds in two
working habits. A run may open with a plan: if the model's first
reply starts with `PLAN:`, the loop records it in the run state and
the ledger, then tells the model to begin. Skipping the plan and
acting directly works exactly as before, and a resumed run keeps its
recorded plan instead of planning again. At the other end, a final
answer is not accepted on sight: the loop shows the model the task
and its draft and asks it to check for errors, unmet requirements,
and unsupported claims. The model replies with a pass marker or one
revised answer. An unparseable check reply counts as a pass, so the
offline stub and small models never break on the check.

**Run state and resume.** Tool loops write their progress after
every step (task, contract, completed tool calls with results,
status) to `botropolis/data/runs/<run_id>.json`
(`botropolis/core/runstate.py`, gitignored). `botropolis resume
<run_id>` rebuilds the conversation from the completed steps and
continues from the first unfinished one; finished tool calls are
shown to the model as observations, not re-executed. A run that
already finished says so and exits.

**Task map.** The same progress, in words. After every step the
loop also writes `botropolis/data/runs/<run_id>.map.md`
(`botropolis/core/taskmap.py`, gitignored): the goal and the
contract's done-when items, the recorded plan with each step marked
done or pending, the tool calls so far with one-line outcomes, and
any blockers. Plan steps are marked done in order as tool calls
succeed, which is a progress sketch, not a proof. A resumed run
reads its map back into the conversation, so the continuing agent
sees where the work stands in words, not just raw steps.
`botropolis map <run_id>` prints it; when the map file is missing
it is rendered from the state file instead.

**Done evidence.** A contract's machine checks are shell commands
with an expected exit code (`botropolis/core/evidence.py`). Before
the CEO finalizes a report for a contracted run, the checks run
inside the agent sandbox and the results ride on the report as
evidence. All passing keeps the report `done`; any failure makes it
`unverified`. The agent claims, the harness verifies.

**Judge.** Machine checks catch what a shell command can catch;
they cannot read whether a report actually meets its contract. So
after the checks pass, a different agent judges it: the Reviewer
(Sofia) reads the report against the contract's done-when items and
exclusions in one single-shot call (`botropolis/core/judge.py`) and
replies `VERDICT: PASS` or `VERDICT: FAIL` with one reason. A
failed verdict flips the report to `unverified` with the reason
attached as evidence, exactly like a failed machine check. A pass
leaves the status as the checks left it. The judge is a direct
model call, never an agent run, so it cannot recurse into judging.
When the judge call errors, its reply cannot be parsed, or there
is no reviewer, the judge is recorded as skipped and the
machine-check result stands; when the machine checks already
failed, the judge does not run. Uncontracted runs are not judged.

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
