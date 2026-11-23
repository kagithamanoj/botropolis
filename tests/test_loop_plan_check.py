"""Plan and verify tests: the tool loop records an opening plan,
and checks a final answer once before accepting it."""

import yaml

from botropolis.core.agent import Agent
from botropolis.core.ledger import Ledger
from botropolis.core.loop import parse_check_reply, parse_plan_reply
from botropolis.core.runstate import RunStore, task_from_state
from botropolis.core.schemas import Task

CALC_ACTION = 'ACTION: {"tool": "calculator", "args": {"expression": "6 * 7"}}'
PLAN = "PLAN: First work out 6 times 7 with the calculator, then answer."


class RecordingClient:
    """Scripted stand-in for ModelClient. Replies play in order and
    every conversation it is shown is kept for inspection."""

    def __init__(self, replies):
        self.replies = list(replies)
        self.calls = 0
        self.last_usage = {"input": 0, "output": 0}
        self.seen = []

    def chat(self, model_id, messages):
        self.calls += 1
        self.seen.append([dict(m) for m in messages])
        if self.calls <= len(self.replies):
            return self.replies[self.calls - 1]
        return self.replies[-1]

    def provider_for(self, model_id):
        return "stub"


class CrashAfterClient(RecordingClient):
    """Plays its replies, then dies like a dead process."""

    def chat(self, model_id, messages):
        if self.calls >= len(self.replies):
            self.calls += 1
            raise RuntimeError("process died")
        return super().chat(model_id, messages)


def make_agent(tmp_path, client, **spec_overrides):
    spec = {
        "name": "TestAgent",
        "title": "Tester",
        "department": "research",
        "specialty": "testing",
        "model": "stub",
        "system_prompt": "You are a test agent.",
        "toolkit": ["calculator"],
    }
    spec.update(spec_overrides)
    path = tmp_path / "testagent.yaml"
    path.write_text(yaml.safe_dump(spec), encoding="utf-8")
    return Agent(path, client=client)


def test_parse_plan_reply():
    assert parse_plan_reply(PLAN) is not None
    assert parse_plan_reply("  \nPLAN: steps follow") is not None
    assert parse_plan_reply("Here is my PLAN: not at the start") is None
    assert parse_plan_reply("FINAL: done") is None
    assert parse_plan_reply("") is None


def test_parse_check_reply_variants():
    assert parse_check_reply("CHECK: PASS", "draft") == ("draft", False)
    assert parse_check_reply("FINAL: fixed", "draft") == ("fixed", True)
    assert parse_check_reply("CHECK: REVISED\nFINAL: fixed", "draft") == (
        "fixed",
        True,
    )
    # Restating the draft word for word is a pass in effect.
    assert parse_check_reply("FINAL: draft", "draft") == ("draft", False)
    # Anything unparseable counts as a pass.
    assert parse_check_reply("looks fine to me", "draft") == ("draft", False)
    assert parse_check_reply("", "draft") == ("draft", False)


def test_plan_is_recorded_and_the_run_continues(tmp_path):
    store = RunStore(root=tmp_path / "runs")
    client = RecordingClient([PLAN, CALC_ACTION, "FINAL: 42", "CHECK: PASS"])
    agent = make_agent(tmp_path, client)
    result = agent.run(
        Task(description="what is 6 times 7", department="research"),
        run_id="run-plan",
        store=store,
    )
    assert result.success
    assert result.output == "42"
    assert [c.tool for c in result.tool_calls] == ["calculator"]
    state = store.load("run-plan")
    assert state["plan"] == PLAN
    assert len(state["steps"]) == 1
    # The call after the plan was told to begin.
    second_call = client.seen[1]
    assert any("Plan recorded. Begin" in m["content"] for m in second_call)


def test_plan_lands_in_the_ledger(tmp_path):
    ledger = Ledger(path=tmp_path / "ledger.jsonl")
    client = RecordingClient([PLAN, "FINAL: done", "CHECK: PASS"])
    agent = make_agent(tmp_path, client)
    result = agent.run("plan a small thing", ledger=ledger, run_id="run-ledger")
    assert result.success
    kinds = [event["kind"] for event in ledger.events("run-ledger")]
    assert "plan_recorded" in kinds
    assert "answer_checked" in kinds


def test_run_without_a_plan_works_as_before(tmp_path):
    store = RunStore(root=tmp_path / "runs")
    client = RecordingClient([CALC_ACTION, "FINAL: 42", "CHECK: PASS"])
    agent = make_agent(tmp_path, client)
    result = agent.run(
        Task(description="what is 6 times 7", department="research"),
        run_id="run-no-plan",
        store=store,
    )
    assert result.success
    assert result.output == "42"
    state = store.load("run-no-plan")
    assert state["plan"] is None


def test_second_plan_is_nudged_not_recorded(tmp_path):
    store = RunStore(root=tmp_path / "runs")
    other_plan = "PLAN: A different plan that arrives too late."
    client = RecordingClient(
        [PLAN, other_plan, "FINAL: done anyway", "CHECK: PASS"]
    )
    agent = make_agent(tmp_path, client)
    result = agent.run(
        Task(description="a task", department="research"),
        run_id="run-two-plans",
        store=store,
    )
    assert result.success
    assert result.output == "done anyway"
    state = store.load("run-two-plans")
    assert state["plan"] == PLAN


def test_check_pass_keeps_the_draft(tmp_path):
    client = RecordingClient(["FINAL: the draft answer", "CHECK: PASS"])
    agent = make_agent(tmp_path, client)
    result = agent.run("answer something")
    assert result.success
    assert result.output == "the draft answer"


def test_check_can_revise_the_answer_once(tmp_path):
    client = RecordingClient(
        ["FINAL: 6 times 7 is 41", "FINAL: 6 times 7 is 42"]
    )
    agent = make_agent(tmp_path, client)
    result = agent.run("what is 6 times 7")
    assert result.success
    assert result.output == "6 times 7 is 42"


def test_check_revision_after_a_check_line(tmp_path):
    client = RecordingClient(
        ["FINAL: wrong", "CHECK: REVISED\nFINAL: right"]
    )
    agent = make_agent(tmp_path, client)
    result = agent.run("a question")
    assert result.success
    assert result.output == "right"


def test_unparseable_check_counts_as_a_pass(tmp_path):
    client = RecordingClient(
        ["FINAL: the draft answer", "I read it again and it seems fine"]
    )
    agent = make_agent(tmp_path, client)
    result = agent.run("answer something")
    assert result.success
    assert result.output == "the draft answer"


def test_resume_keeps_the_recorded_plan(tmp_path):
    store = RunStore(root=tmp_path / "runs")
    crashing = CrashAfterClient([PLAN, CALC_ACTION])
    agent = make_agent(tmp_path, crashing)
    task = Task(description="what is 6 times 7", department="research")
    first = agent.run(task, run_id="run-resume", store=store)
    assert first.success is False
    state = store.load("run-resume")
    assert state["plan"] == PLAN
    assert len(state["steps"]) == 1

    client = RecordingClient(["FINAL: 42", "CHECK: PASS"])
    agent2 = make_agent(tmp_path, client)
    resumed_task = task_from_state(state)
    result = agent2.run(
        resumed_task, run_id="run-resume", store=store, resume_state=state
    )
    assert result.success
    assert result.output == "42"
    # The finished calculator call was not redone.
    assert len(result.tool_calls) == 1
    # The resumed conversation opens with the recorded plan.
    first_call = client.seen[0]
    contents = [m["content"] for m in first_call]
    assert any(PLAN in content for content in contents)
    assert any("Plan recorded. Begin" in content for content in contents)
