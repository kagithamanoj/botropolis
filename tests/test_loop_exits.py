"""Loop exit tests: same-error exit, token and time budgets, and
proof that normal runs are unaffected."""

import time

import yaml

from botropolis.core.agent import Agent

BAD_EXPR = 'ACTION: {"tool": "calculator", "args": {"expression": "2 +"}}'
OTHER_BAD_EXPR = 'ACTION: {"tool": "calculator", "args": {"expression": "foo("}}'
GOOD_EXPR = 'ACTION: {"tool": "calculator", "args": {"expression": "1 + 1"}}'


class FakeClient:
    """Scripted stand-in for ModelClient. Replies play in order."""

    def __init__(self, replies, usage_per_call=None, delay=0.0):
        self.replies = list(replies)
        self.calls = 0
        self.last_usage = {"input": 0, "output": 0}
        self.usage_per_call = usage_per_call or {"input": 0, "output": 0}
        self.delay = delay

    def chat(self, model_id, messages):
        self.calls += 1
        if self.delay:
            time.sleep(self.delay)
        self.last_usage = dict(self.usage_per_call)
        if self.calls <= len(self.replies):
            return self.replies[self.calls - 1]
        return self.replies[-1]

    def provider_for(self, model_id):
        return "stub"


def make_agent(tmp_path, replies, client_kwargs=None, **spec_overrides):
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
    client = FakeClient(replies, **(client_kwargs or {}))
    return Agent(path, client=client), client


def test_same_error_twice_stops_the_loop(tmp_path):
    agent, client = make_agent(
        tmp_path, [BAD_EXPR, BAD_EXPR, GOOD_EXPR, "FINAL: never reached"]
    )
    result = agent.run("do math")
    assert result.success is False
    assert result.metadata["escalated"] is True
    assert "calculator" in result.metadata["escalation_reason"]
    assert "twice in a row" in result.metadata["escalation_reason"]
    assert len(result.tool_calls) == 2
    # The loop stopped at the second failure: no more model calls.
    assert client.calls == 2


def test_different_errors_do_not_escalate(tmp_path):
    agent, client = make_agent(
        tmp_path, [BAD_EXPR, OTHER_BAD_EXPR, "FINAL: recovered"]
    )
    result = agent.run("do math")
    assert result.success is True
    assert result.output == "recovered"
    assert len(result.tool_calls) == 2
    assert not result.metadata.get("escalated")


def test_same_error_not_in_a_row_does_not_escalate(tmp_path):
    agent, client = make_agent(
        tmp_path, [BAD_EXPR, GOOD_EXPR, BAD_EXPR, "FINAL: fine"]
    )
    result = agent.run("do math")
    assert result.success is True
    assert result.output == "fine"
    assert len(result.tool_calls) == 3


def test_token_budget_from_spec_stops_the_run(tmp_path):
    agent, client = make_agent(
        tmp_path,
        [GOOD_EXPR, GOOD_EXPR, GOOD_EXPR, "FINAL: never reached"],
        client_kwargs={"usage_per_call": {"input": 600, "output": 0}},
        token_budget=1000,
    )
    result = agent.run("do math")
    assert result.success is False
    assert result.metadata["escalated"] is True
    assert "token budget" in result.metadata["escalation_reason"]
    # First call used 600 (fine, tool ran), second pushed it to 1200
    # and the run stopped before acting on that reply.
    assert len(result.tool_calls) == 1
    assert client.calls == 2


def test_token_budget_as_run_param_overrides(tmp_path):
    agent, client = make_agent(
        tmp_path,
        [GOOD_EXPR, "FINAL: never reached"],
        client_kwargs={"usage_per_call": {"input": 600, "output": 0}},
    )
    result = agent.run("do math", token_budget=100)
    assert result.success is False
    assert result.metadata["escalated"] is True
    assert "token budget" in result.metadata["escalation_reason"]
    # Budget blown by the first model call: the tool never ran.
    assert len(result.tool_calls) == 0


def test_time_budget_stops_the_run(tmp_path):
    agent, client = make_agent(
        tmp_path,
        [GOOD_EXPR, "FINAL: never reached"],
        client_kwargs={"delay": 0.2},
        time_budget_seconds=0.05,
    )
    result = agent.run("do math")
    assert result.success is False
    assert result.metadata["escalated"] is True
    assert "time budget" in result.metadata["escalation_reason"]
    assert len(result.tool_calls) == 0


def test_budgets_not_hit_leave_normal_runs_alone(tmp_path):
    agent, client = make_agent(
        tmp_path,
        [GOOD_EXPR, "FINAL: 2"],
        client_kwargs={"usage_per_call": {"input": 10, "output": 5}},
        token_budget=100000,
        time_budget_seconds=60,
    )
    result = agent.run("do math")
    assert result.success is True
    assert result.output == "2"
    assert len(result.tool_calls) == 1
    assert not result.metadata.get("escalated")


def test_no_budgets_configured_by_default(tmp_path):
    agent, _ = make_agent(tmp_path, ["FINAL: ok"])
    assert agent.token_budget is None
    assert agent.time_budget_seconds is None


def test_spec_budgets_are_parsed(tmp_path):
    agent, _ = make_agent(
        tmp_path, ["FINAL: ok"], token_budget=5000, time_budget_seconds=30
    )
    assert agent.token_budget == 5000
    assert agent.time_budget_seconds == 30.0
