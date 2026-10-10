"""Context tests: the typed object agents share, replacing Dict[str, Any]."""

import yaml

from botropolis.core.agent import Agent
from botropolis.core.orchestrator import CEO, history_context
from botropolis.core.registry import AgentRegistry
from botropolis.core.schemas import Context, Task


class FakeClient:
    def __init__(self, replies):
        self.replies = list(replies)
        self.calls = 0
        self.last_usage = {"input": 0, "output": 0}

    def chat(self, model_id, messages):
        self.calls += 1
        return self.replies[-1]

    def provider_for(self, model_id):
        return "stub"


def make_agent(tmp_path):
    spec = {
        "name": "TestAgent",
        "title": "Tester",
        "department": "ops",
        "specialty": "testing",
        "model": "stub",
        "system_prompt": "You are a test agent.",
    }
    path = tmp_path / "testagent.yaml"
    path.write_text(yaml.safe_dump(spec), encoding="utf-8")
    return Agent(path, client=FakeClient(["[offline stub] ok"]))


# ── the Context type ────────────────────────────────────────────────────

def test_empty_context_is_falsy_and_serializes_to_empty_dict():
    ctx = Context()
    assert not ctx
    assert ctx.to_dict() == {}


def test_to_dict_omits_empty_fields():
    ctx = Context(chat_history="user: hi")
    assert ctx
    assert ctx.to_dict() == {"chat_history": "user: hi"}


def test_extra_dict_passes_through():
    ctx = Context(extra={"source": "war-room"})
    assert ctx.to_dict() == {"source": "war-room"}


def test_task_defaults_to_empty_context():
    task = Task(description="do it")
    assert isinstance(task.context, Context)
    assert not task.context


def test_task_to_dict_serializes_context_cleanly():
    task = Task(description="do it", context=Context(chat_history="user: hi"))
    assert task.to_dict()["context"] == {"chat_history": "user: hi"}


def test_task_to_dict_with_empty_context():
    task = Task(description="do it")
    assert task.to_dict()["context"] == {}


# ── builders ────────────────────────────────────────────────────────────

def test_history_context_returns_typed_context():
    history = [
        {"role": "user", "content": "What is our Q3 revenue?"},
        {"role": "assistant", "content": "Q3 revenue was $4.2M."},
    ]
    ctx = history_context(history)
    assert isinstance(ctx, Context)
    assert "user: What is our Q3 revenue?" in ctx.chat_history
    assert "assistant: Q3 revenue was $4.2M." in ctx.chat_history


def test_history_context_empty_history_gives_empty_context():
    assert not history_context(None)
    assert not history_context([])
    assert not history_context([{"role": "user", "content": "   "}])


def test_team_context_packs_teammate_outputs():
    ceo = CEO(AgentRegistry())
    prior = [(1, "coder", "draft code here"), (1, "reviewer", "looks good")]
    ctx = ceo._team_context(prior)
    assert isinstance(ctx, Context)
    assert "[round 1 - coder]" in ctx.teammates_so_far
    assert "draft code here" in ctx.teammates_so_far
    assert not ceo._team_context([])


# ── prompt rendering ────────────────────────────────────────────────────

def test_build_messages_renders_context_lines(tmp_path):
    agent = make_agent(tmp_path)
    task = Task(description="follow up", context=Context(chat_history="user: hi"))
    user_msg = agent.build_messages(task)[1]["content"]
    assert "- chat_history: user: hi" in user_msg


def test_build_messages_omits_empty_context(tmp_path):
    agent = make_agent(tmp_path)
    task = Task(description="follow up")
    user_msg = agent.build_messages(task)[1]["content"]
    assert "Context:" not in user_msg


def test_end_to_end_history_reaches_agent_prompt(tmp_path):
    agent = make_agent(tmp_path)
    ceo = CEO(AgentRegistry())
    tasks = ceo.plan(
        "And Q4?",
        history=[{"role": "user", "content": "What is our Q3 revenue?"}],
    )
    user_msg = agent.build_messages(tasks[0])[1]["content"]
    assert "user: What is our Q3 revenue?" in user_msg
