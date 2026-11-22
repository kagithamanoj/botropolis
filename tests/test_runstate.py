"""Run state tests: progress is written after every step, and a
run interrupted mid-way resumes without redoing finished tool calls."""

import yaml

from botropolis.cli import main
from botropolis.core.agent import Agent
from botropolis.core.runstate import RunStore, task_from_state
from botropolis.core.schemas import Contract, Task
from botropolis.tools import get_tool

CALC_ACTION = 'ACTION: {"tool": "calculator", "args": {"expression": "6 * 7"}}'


class ScriptedClient:
    """Replies play in order; records the messages it was shown."""

    def __init__(self, replies):
        self.replies = list(replies)
        self.calls = 0
        self.last_usage = {"input": 0, "output": 0}
        self.seen_messages = []

    def chat(self, model_id, messages):
        self.calls += 1
        self.seen_messages = messages
        if self.calls <= len(self.replies):
            return self.replies[self.calls - 1]
        return self.replies[-1]

    def provider_for(self, model_id):
        return "stub"


class CrashingClient:
    """Answers once with a tool call, then dies like a dead process."""

    def __init__(self):
        self.calls = 0
        self.last_usage = {"input": 0, "output": 0}

    def chat(self, model_id, messages):
        self.calls += 1
        if self.calls == 1:
            return CALC_ACTION
        raise RuntimeError("process died")

    def provider_for(self, model_id):
        return "stub"


def make_agent(tmp_path, client):
    spec = {
        "name": "TestAgent",
        "title": "Tester",
        "department": "research",
        "specialty": "testing",
        "model": "stub",
        "system_prompt": "You are a test agent.",
        "toolkit": ["calculator"],
    }
    path = tmp_path / "testagent.yaml"
    path.write_text(yaml.safe_dump(spec), encoding="utf-8")
    return Agent(path, client=client)


def test_store_round_trip(tmp_path):
    store = RunStore(root=tmp_path)
    store.save({"run_id": "abc123", "status": "running", "steps": []})
    loaded = store.load("abc123")
    assert loaded["status"] == "running"
    assert store.exists("abc123")
    assert not store.exists("nope")


def test_missing_run_raises_file_not_found(tmp_path):
    store = RunStore(root=tmp_path)
    try:
        store.load("nope")
    except FileNotFoundError:
        pass
    else:
        raise AssertionError("expected FileNotFoundError")


def test_finished_run_writes_state_with_steps(tmp_path):
    store = RunStore(root=tmp_path / "runs")
    agent = make_agent(tmp_path, ScriptedClient([CALC_ACTION, "FINAL: 42"]))
    result = agent.run(
        Task(description="what is 6 times 7", department="research"),
        run_id="run-finished",
        store=store,
    )
    assert result.success
    state = store.load("run-finished")
    assert state["status"] == "finished"
    assert state["answer"] == "42"
    assert state["agent"] == "TestAgent"
    assert len(state["steps"]) == 1
    assert state["steps"][0]["tool"] == "calculator"
    assert state["steps"][0]["result_preview"] == "42"


def test_interrupted_run_keeps_partial_state(tmp_path):
    store = RunStore(root=tmp_path / "runs")
    agent = make_agent(tmp_path, CrashingClient())
    result = agent.run(
        Task(description="what is 6 times 7", department="research"),
        run_id="run-crashed",
        store=store,
    )
    assert result.success is False
    state = store.load("run-crashed")
    assert state["status"] == "running"
    assert len(state["steps"]) == 1
    assert state["steps"][0]["result_preview"] == "42"


def test_resume_does_not_redo_completed_tool_calls(tmp_path, monkeypatch):
    store = RunStore(root=tmp_path / "runs")
    crashed = make_agent(tmp_path, CrashingClient())
    crashed.run(
        Task(description="what is 6 times 7", department="research"),
        run_id="run-resume",
        store=store,
    )
    state = store.load("run-resume")
    assert state["status"] == "running"

    executions = []
    calc = get_tool("calculator")
    real_execute = calc.execute

    def counting_execute(**kwargs):
        executions.append(kwargs)
        return real_execute(**kwargs)

    monkeypatch.setattr(calc, "execute", counting_execute)

    client = ScriptedClient(["FINAL: 42, from the earlier calculation"])
    resumed = make_agent(tmp_path, client)
    result = resumed.run(
        task_from_state(state),
        run_id="run-resume",
        store=store,
        resume_state=state,
    )
    assert result.success
    assert result.output == "42, from the earlier calculation"
    # The finished calculator call was not executed again.
    assert executions == []
    # The result still carries the full tool history.
    assert len(result.tool_calls) == 1
    assert result.tool_calls[0].tool == "calculator"
    # The model was shown the earlier observation, so it could answer
    # from it instead of recomputing.
    convo_text = "\n".join(m["content"] for m in client.seen_messages)
    assert "OBSERVATION: 42" in convo_text
    # And the state file now shows the run finished.
    assert store.load("run-resume")["status"] == "finished"


def test_resume_continues_with_new_steps(tmp_path):
    store = RunStore(root=tmp_path / "runs")
    crashed = make_agent(tmp_path, CrashingClient())
    crashed.run(
        Task(description="what is 6 times 7, then double it", department="research"),
        run_id="run-continue",
        store=store,
    )
    state = store.load("run-continue")
    client = ScriptedClient(
        [
            'ACTION: {"tool": "calculator", "args": {"expression": "42 * 2"}}',
            "FINAL: 84",
        ]
    )
    resumed = make_agent(tmp_path, client)
    result = resumed.run(
        task_from_state(state),
        run_id="run-continue",
        store=store,
        resume_state=state,
    )
    assert result.success
    assert result.output == "84"
    assert len(result.tool_calls) == 2
    final_state = store.load("run-continue")
    assert final_state["status"] == "finished"
    assert len(final_state["steps"]) == 2


def test_state_records_the_contract(tmp_path):
    store = RunStore(root=tmp_path / "runs")
    agent = make_agent(tmp_path, ScriptedClient(["FINAL: done"]))
    contract = Contract(goal="Multiply things", exclusions=["division"])
    agent.run(
        Task(description="multiply", department="research", contract=contract),
        run_id="run-contract",
        store=store,
    )
    state = store.load("run-contract")
    assert state["contract"]["goal"] == "Multiply things"
    rebuilt = task_from_state(state)
    assert rebuilt.contract == contract


def test_cli_resume_finished_run_exits_cleanly(tmp_path, monkeypatch, capsys):
    monkeypatch.setenv("BOTROPOLIS_RUNS_DIR", str(tmp_path))
    store = RunStore(root=tmp_path)
    store.save(
        {
            "run_id": "done1",
            "agent": "pipelines",
            "task": {"description": "x"},
            "steps": [],
            "status": "finished",
            "answer": "the final answer",
        }
    )
    assert main(["resume", "done1"]) == 0
    out = capsys.readouterr().out
    assert "already finished" in out
    assert "the final answer" in out


def test_cli_resume_unknown_run_fails(tmp_path, monkeypatch, capsys):
    monkeypatch.setenv("BOTROPOLIS_RUNS_DIR", str(tmp_path))
    assert main(["resume", "missing"]) == 1
    assert "No saved run" in capsys.readouterr().out


def test_cli_ask_saves_state_and_prints_run_id(tmp_path, monkeypatch, capsys):
    monkeypatch.setenv("BOTROPOLIS_RUNS_DIR", str(tmp_path))
    assert main(["ask", "pipelines", "say hi"]) == 0
    out = capsys.readouterr().out
    assert "run id:" in out
    run_id = out.strip().splitlines()[-1].replace("run id:", "").strip()
    state = RunStore(root=tmp_path).load(run_id)
    assert state["agent"] == "pipelines"
    assert state["status"] == "finished"
