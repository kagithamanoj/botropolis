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
 Aarav Meera Dev ...
```

## Manoj, the CEO (orchestrator)

`botropolis/core/orchestrator.py` holds the `CEO` class. It does three things:

1. **Plan.** Scores the request against keyword lists per department and
   picks up to three relevant departments. No keywords hit, research gets
   the job by default.
2. **Assign.** Picks the lead agent for each chosen department
   (`LEAD_AGENT` map), or the first agent alphabetically as a fallback.
3. **Report.** Runs each agent, collects `AgentResult` objects, and builds
   a `CompanyReport` with an executive summary.

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

`botropolis/tools/builtin.py` defines the `Tool` base class and five
builtin tools: `web_search`, `calculator`, `read_file`, `write_file`,
`current_time`. Agents declare which tools they may use in their spec.
`web_search` is stub-backed until you wire a real provider; the schema is
stable so swapping the implementation is a one function change.

## Schemas

`botropolis/core/schemas.py` has the three dataclasses everything passes
around: `Task` (work to do), `AgentResult` (what one agent produced), and
`CompanyReport` (the CEO's final answer). All three serialize with
`to_dict()`, which is what the API server returns.

## Server

`botropolis/server.py` is a thin FastAPI layer over the CEO: `POST /ask`
takes a request string and returns a `CompanyReport` as JSON. The web
layer does no reasoning of its own.

## Training

`training/` is separate from the runtime on purpose. Datasets, configs,
the plan-only trainer runner, and the eval harness live there. A trained
model re-enters the system by being registered in `models/registry.yaml`
and referenced from an agent spec. See `docs/training-guide.md`.
