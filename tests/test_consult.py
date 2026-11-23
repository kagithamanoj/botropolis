"""Consult tool tests: a specialist can be asked mid-task, and the
guards hold: no self-consults, no chains, a per-run budget, and a
clean error for an unknown name."""

import pytest
import yaml

from botropolis.core.agent import Agent
from botropolis.core.registry import AgentRegistry
from botropolis.tools import TOOLS, get_tool
from botropolis.tools.consult import set_consult_registry

CONSULT_SPECIALIST = (
    'ACTION: {"tool": "consult", "args": {"agent": "specialist", '
    '"question": "When does the staging freeze start?"}}'
)
SPECIALIST_ANSWER = "FINAL: The staging freeze starts on Fridays at 5pm"


class ScriptedClient:
    """One reply queue shared by every agent in the test, so the
    interleaving of a run and its consults is deterministic."""

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


def _write_spec(agents_dir, spec):
    dept_dir = agents_dir / spec["department"]
    dept_dir.mkdir(parents=True, exist_ok=True)
    path = dept_dir / f"{spec['name']}.yaml"
    path.write_text(yaml.safe_dump(spec), encoding="utf-8")
    return path


def _spec(name, toolkit=None):
    spec = {
        "name": name,
        "title": f"{name} title",
        "department": "research",
        "specialty": "testing",
        "model": "stub",
        "system_prompt": f"You are {name}, a test agent.",
    }
    if toolkit is not None:
        spec["toolkit"] = toolkit
    return spec


@pytest.fixture()
def company(tmp_path):
    """A primary agent plus a registry holding a specialist and a
    fellow consult user, all on one scripted client."""
    agents_dir = tmp_path / "agents"
    _write_spec(agents_dir, _spec("specialist"))
    _write_spec(agents_dir, _spec("chainer", toolkit=["consult"]))

    def build(replies):
        client = ScriptedClient(replies)
        registry = AgentRegistry(agents_dir=agents_dir, client=client)
        set_consult_registry(registry)
        primary_path = _write_spec(
            tmp_path / "primary", _spec("primary", toolkit=["consult"])
        )
        primary = Agent(primary_path, client=client)
        return primary, client

    yield build
    set_consult_registry(None)


def test_consult_tool_is_registered():
    assert "consult" in TOOLS
    assert get_tool("consult").name == "consult"


def test_every_toolkit_agent_can_consult():
    registry = AgentRegistry()
    for agent in registry.list_all():
        if agent.toolkit:
            assert "consult" in agent.toolkit, agent.name


def test_successful_consult_returns_the_specialists_answer(company):
    primary, client = company(
        [
            CONSULT_SPECIALIST,
            SPECIALIST_ANSWER,
            "FINAL: Per the specialist, the freeze starts Fridays at 5pm",
            "CHECK: PASS",
        ]
    )
    result = primary.run("When does the staging freeze start?")
    assert result.success
    assert result.output == "Per the specialist, the freeze starts Fridays at 5pm"
    assert len(result.tool_calls) == 1
    call = result.tool_calls[0]
    assert call.tool == "consult"
    assert call.success
    # The recorded call keeps the model's own args; the injected
    # caller name and run context are execution details.
    assert call.args == {
        "agent": "specialist",
        "question": "When does the staging freeze start?",
    }
    assert "Fridays at 5pm" in call.result_preview
    assert '"agent": "specialist"' in call.result_preview


def test_self_consult_is_blocked(company):
    primary, client = company(
        [
            'ACTION: {"tool": "consult", "args": {"agent": "Primary", '
            '"question": "What do I think?"}}',
            "FINAL: Answered it myself",
            "CHECK: PASS",
        ]
    )
    result = primary.run("A question")
    assert result.success
    assert result.output == "Answered it myself"
    assert "cannot consult yourself" in result.tool_calls[0].result_preview
    # No sub-run happened: three calls, all the primary's own.
    assert client.calls == 3


def test_unknown_agent_errors_cleanly(company):
    primary, client = company(
        [
            'ACTION: {"tool": "consult", "args": {"agent": "ghost", '
            '"question": "Boo?"}}',
            "FINAL: Nobody by that name works here",
            "CHECK: PASS",
        ]
    )
    result = primary.run("Ask the ghost")
    assert result.success
    assert result.output == "Nobody by that name works here"
    assert "No agent named 'ghost'" in result.tool_calls[0].result_preview


def test_consults_do_not_chain(company):
    primary, client = company(
        [
            'ACTION: {"tool": "consult", "args": {"agent": "chainer", '
            '"question": "When does the staging freeze start?"}}',
            'ACTION: {"tool": "consult", "args": {"agent": "specialist", '
            '"question": "When does the staging freeze start?"}}',
            "FINAL: I had to answer it myself: Fridays at 5pm",
            "CHECK: PASS",
            "FINAL: The chainer answered: Fridays at 5pm",
            "CHECK: PASS",
        ]
    )
    result = primary.run("When does the staging freeze start?")
    assert result.success
    assert result.output == "The chainer answered: Fridays at 5pm"
    # The chainer's own consult attempt was refused with the depth
    # error, which the chainer saw as its observation.
    seen_text = "\n".join(
        message["content"] for call in client.seen for message in call
    )
    assert "Consults do not chain" in seen_text


def test_consult_budget_is_enforced(company):
    consult = (
        'ACTION: {"tool": "consult", "args": {"agent": "specialist", '
        '"question": "When does the staging freeze start?"}}'
    )
    primary, client = company(
        [
            consult,
            SPECIALIST_ANSWER,
            consult,
            SPECIALIST_ANSWER,
            consult,
            SPECIALIST_ANSWER,
            consult,
            "FINAL: Three answers was enough",
            "CHECK: PASS",
        ]
    )
    result = primary.run("Keep asking the specialist")
    assert result.success
    assert len(result.tool_calls) == 4
    for call in result.tool_calls[:3]:
        assert "Fridays at 5pm" in call.result_preview
    assert "budget" in result.tool_calls[3].result_preview
    # The fourth consult never reached the specialist: nine calls in
    # total, and the queue was consumed exactly.
    assert client.calls == 9


def test_tool_guards_directly(company):
    company([])  # pins the registry for this test
    tool = get_tool("consult")
    fresh = {"depth": 0, "consults_used": 0}
    out = tool.execute(
        agent="specialist", question="q", caller="primary",
        run_context={"depth": 1, "consults_used": 0},
    )
    assert "do not chain" in out["error"]
    out = tool.execute(
        agent="specialist", question="q", caller="primary",
        run_context={"depth": 0, "consults_used": 3},
    )
    assert "budget" in out["error"]
    out = tool.execute(agent="", question="q", caller="primary", run_context=fresh)
    assert "error" in out
    out = tool.execute(
        agent="specialist", question="", caller="primary", run_context=fresh
    )
    assert "error" in out
