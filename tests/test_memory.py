"""Memory tests: the store, the tools, the toolkits, and CEO recall."""

import pytest
import yaml

from botropolis.core.agent import Agent
from botropolis.core.loop import execute_tool_call
from botropolis.core.memory import MemoryStore, set_memory_store
from botropolis.core.orchestrator import CEO, memory_context
from botropolis.core.registry import AgentRegistry
from botropolis.core.schemas import Context, Task
from botropolis.tools import TOOLS, get_tool


@pytest.fixture()
def store():
    return MemoryStore()


@pytest.fixture()
def pinned_store(store):
    """Pin an in-memory store as the shared one for tool calls."""
    set_memory_store(store)
    try:
        yield store
    finally:
        set_memory_store(None)


# ── the store ───────────────────────────────────────────────────────────

def test_save_and_recall_round_trip(store):
    saved = store.save(
        "Q3 revenue was $4.2M", agent="analyst", importance=4, tags=["finance"]
    )
    assert saved.id
    assert saved.access_count == 0
    found = store.recall("Q3 revenue")
    assert [m.text for m in found] == ["Q3 revenue was $4.2M"]
    assert found[0].agent == "analyst"
    assert found[0].tags == ["finance"]


def test_recall_without_overlap_returns_nothing(store):
    store.save("The office is in Austin", agent="scout")
    assert store.recall("quarterly tax filing") == []


def test_importance_breaks_ties_in_ranking(store):
    store.save("Deploy window is Friday", agent="devops", importance=1)
    store.save("Deploy freeze starts Friday", agent="devops", importance=5)
    found = store.recall("deploy friday")
    assert found[0].text == "Deploy freeze starts Friday"
    assert found[1].text == "Deploy window is Friday"


def test_keyword_overlap_drives_ranking(store):
    store.save("The budget review is monthly", agent="markets", importance=3)
    store.save("The budget review covers hiring", agent="markets", importance=3)
    found = store.recall("budget review hiring")
    assert found[0].text == "The budget review covers hiring"


def test_newer_memory_wins_an_exact_tie(store):
    store.save("Standup is at nine", agent="scheduler", importance=3)
    store.save("Standup moved to ten", agent="scheduler", importance=3)
    found = store.recall("standup")
    assert found[0].text == "Standup moved to ten"


def test_tags_are_searchable(store):
    store.save("Renewal is in March", agent="inbox", tags=["acme", "contract"])
    found = store.recall("acme")
    assert [m.text for m in found] == ["Renewal is in March"]


def test_recall_updates_access_stats(store):
    store.save("Backups run nightly", agent="devops")
    store.save("Lunch is at noon", agent="scheduler")
    found = store.recall("backups")
    assert found[0].access_count == 1
    assert found[0].last_accessed_at is not None
    by_text = {m.text: m for m in store.all()}
    assert by_text["Backups run nightly"].access_count == 1
    assert by_text["Lunch is at noon"].access_count == 0
    store.recall("backups")
    assert {m.text: m for m in store.all()}["Backups run nightly"].access_count == 2


def test_importance_is_clamped_to_one_to_five(store):
    assert store.save("x fact", importance=99).importance == 5
    assert store.save("y fact", importance=0).importance == 1


def test_store_persists_to_disk(tmp_path):
    path = tmp_path / "memory.db"
    first = MemoryStore(path)
    first.save("The CEO is named Manoj", agent="scout", importance=5)
    first.close()
    second = MemoryStore(path)
    assert [m.text for m in second.recall("CEO Manoj")] == ["The CEO is named Manoj"]
    second.close()


# ── the tools ───────────────────────────────────────────────────────────

def test_memory_tools_are_registered():
    assert "memory_save" in TOOLS
    assert "memory_recall" in TOOLS
    assert get_tool("memory_save").name == "memory_save"
    assert get_tool("memory_recall").name == "memory_recall"


def test_memory_save_and_recall_tools(pinned_store):
    out = get_tool("memory_save").execute(
        fact="Acme prefers email over calls", importance=4, tags="acme, contact"
    )
    assert out["saved"] is True
    assert out["memory"]["tags"] == ["acme", "contact"]
    found = get_tool("memory_recall").execute(query="acme contact")
    assert found["count"] == 1
    assert found["memories"][0]["text"] == "Acme prefers email over calls"


def test_memory_save_rejects_an_empty_fact(pinned_store):
    out = get_tool("memory_save").execute(fact="   ")
    assert "error" in out
    assert pinned_store.count() == 0


def test_memory_save_records_the_calling_agent(pinned_store):
    call = execute_tool_call(
        ["memory_save"],
        "memory_save",
        {"fact": "The staging freeze starts Fridays at 5pm", "importance": 4},
        agent_name="scheduler",
    )
    assert call.success
    # The recorded call keeps the model's own args: no injected agent.
    assert "agent" not in call.args
    saved = pinned_store.all()
    assert len(saved) == 1
    assert saved[0].agent == "scheduler"


def test_toolkit_agents_have_memory_tools_and_others_do_not():
    registry = AgentRegistry()
    with_toolkit = []
    without_toolkit = []
    for agent in registry.list_all():
        if agent.toolkit:
            with_toolkit.append(agent)
        else:
            without_toolkit.append(agent)
    assert with_toolkit and without_toolkit
    for agent in with_toolkit:
        assert "memory_save" in agent.toolkit, agent.name
        assert "memory_recall" in agent.toolkit, agent.name
    for agent in without_toolkit:
        assert agent.toolkit == []


# ── CEO auto-recall ─────────────────────────────────────────────────────

def test_memory_context_packs_matching_facts(store):
    store.save("Q3 revenue was $4.2M", agent="analyst", importance=4)
    store.save("The office coffee is decaf", agent="scout")
    packed = memory_context(store, "What was our Q3 revenue?")
    assert "[analyst] Q3 revenue was $4.2M" in packed
    assert "coffee" not in packed
    assert memory_context(None, "anything") == ""
    assert memory_context(store, "unrelated words here") == ""


def test_ceo_attaches_recalled_memories_to_tasks(store):
    store.save("Q3 revenue was $4.2M", agent="analyst", importance=4)
    ceo = CEO(AgentRegistry(), memory=store)
    tasks = ceo.plan("What was our Q3 revenue?")
    assert tasks
    for task in tasks:
        assert "Q3 revenue was $4.2M" in task.context.memories


def test_ceo_without_a_store_attaches_no_memories():
    ceo = CEO(AgentRegistry())
    tasks = ceo.plan("What was our Q3 revenue?")
    assert all(not task.context.memories for task in tasks)
    assert all("memories" not in task.context.to_dict() for task in tasks)


def test_memories_reach_the_agent_prompt(tmp_path):
    class FakeClient:
        last_usage = {"input": 0, "output": 0}

        def chat(self, model_id, messages):
            return "[offline stub] ok"

        def provider_for(self, model_id):
            return "stub"

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
    agent = Agent(path, client=FakeClient())
    task = Task(
        description="follow up",
        context=Context(memories="- [analyst] Q3 revenue was $4.2M"),
    )
    user_msg = agent.build_messages(task)[1]["content"]
    assert "- memories: - [analyst] Q3 revenue was $4.2M" in user_msg
