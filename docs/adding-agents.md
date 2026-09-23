# Adding Agents

New hires welcome. Here is the full process, start to finish.

## 1. Pick a department

Look at `botropolis/agents/` and find the closest fit: research, health,
finance, code, data, legal, marketing, ops, security, support. If nothing
fits, create a new directory. It becomes a department automatically, the
registry discovers `agents/*/*.yaml`.

## 2. Write the spec

Copy an existing spec as a template. Every spec needs these fields:

```yaml
name: TravelPlanner            # unique, used for lookup
title: Travel Planning Specialist
department: ops                # must match the directory name
specialty: Building practical trip itineraries within a budget
model: gpt-4o                  # a model id from models/registry.yaml
tools:                         # subset of the builtin tools
  - web_search
  - calculator
  - current_time
system_prompt: >-
  You are TravelPlanner, ...
example_tasks:
  - Plan a 4 day trip to Chicago under 1200 dollars
```

## 3. Write a good system prompt

This is the part that matters most. What I have learned writing the first
twenty:

- **Name the role and the job in the first sentence.** The model should
  know who it is before anything else.
- **Describe the method, not just the goal.** "Break every question into
  searchable sub-questions" beats "do great research".
- **State the output shape.** Headings, tables, short lists. Vague
  prompts get rambling answers.
- **Include the limits.** Every agent should know what it must not do.
  Health and legal agents carry explicit disclaimers; yours should name
  its own boundaries too.
- **Keep it 3 to 6 sentences.** Long enough to be specific, short enough
  that the model actually follows all of it.

## 4. Register and test

No registration step beyond saving the file. The registry picks it up on
the next run. Verify:

```bash
python -c "
from botropolis.core.registry import AgentRegistry
r = AgentRegistry()
print(r.get('TravelPlanner').describe())
"
python -m pytest tests/test_registry.py -q
```

If you want to add an agent from code instead of a file, see
`examples/add_agent.py`.

## 5. Wire it into routing (optional)

If the CEO should route requests to the new agent, check two maps in
`botropolis/core/orchestrator.py`:

- `DEPARTMENT_KEYWORDS`: add trigger words for the department.
- `LEAD_AGENT`: set the department's lead agent if the new one should
  take point.

Then add a routing test in `tests/test_orchestrator.py`. That is it, the
agent is now a full employee.
