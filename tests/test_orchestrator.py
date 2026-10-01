"""Orchestrator tests: the CEO routes work to the right departments."""

import pytest

from botropolis.core.orchestrator import CEO
from botropolis.core.registry import AgentRegistry


@pytest.fixture(scope="module")
def ceo():
    return CEO(AgentRegistry())


def test_research_question_goes_to_research(ceo):
    report = ceo.handle("Research the current state of small language models")
    assert "research" in report.departments_involved
    assert report.results


def test_code_question_goes_to_code(ceo):
    report = ceo.handle("Write a Python function that reverses a string")
    assert "code" in report.departments_involved


def test_report_contains_agent_results(ceo):
    report = ceo.handle("Explain how compound interest works")
    assert len(report.results) >= 1
    for result in report.results:
        assert result.agent_name
        assert result.department
        assert result.success
        assert result.output  # stub output, clearly labeled offline


def test_every_result_agent_exists_in_registry(ceo):
    report = ceo.handle("Help me plan a budget for next month")
    for result in report.results:
        assert result.agent_name in ceo.registry


def test_summary_mentions_departments(ceo):
    report = ceo.handle("Audit my API for security vulnerabilities")
    assert "security" in report.summary.lower()
    assert report.departments_involved


def test_plan_attaches_chat_history_to_tasks(ceo):
    history = [
        {"role": "user", "content": "What is our Q3 revenue?"},
        {"role": "assistant", "content": "Q3 revenue was $4.2M."},
    ]
    tasks = ceo.plan("And Q4?", history=history)
    assert tasks
    for task in tasks:
        ctx = task.context.get("chat_history", "")
        assert "user: What is our Q3 revenue?" in ctx
        assert "assistant: Q3 revenue was $4.2M." in ctx


def test_plan_without_history_has_no_chat_context(ceo):
    tasks = ceo.plan("Hello")
    assert all("chat_history" not in t.context for t in tasks)


def test_plan_caps_history_turns(ceo):
    history = [{"role": "user", "content": f"turn {i}"} for i in range(30)]
    tasks = ceo.plan("follow up", history=history)
    ctx = tasks[0].context["chat_history"]
    assert "turn 0" not in ctx
    assert "turn 29" in ctx
    assert len(ctx.splitlines()) == 10


def test_handle_accepts_history(ceo):
    history = [{"role": "user", "content": "Remind me what we discussed."}]
    report = ceo.handle("Summarize it", history=history)
    assert report.results


def test_followup_routes_with_history(ceo):
    history = [
        {"role": "user", "content": "What are the symptoms of the flu?"},
        {"role": "assistant", "content": "Fever, cough, fatigue."},
    ]
    tasks = ceo.plan("And how long does it usually last?", history=history)
    assert tasks
    assert all(t.department == "health" for t in tasks)


def test_new_topic_overrides_history(ceo):
    history = [
        {"role": "user", "content": "What are the symptoms of the flu?"},
    ]
    tasks = ceo.plan("Write a python script to parse logs", history=history)
    assert tasks
    assert tasks[0].department == "code"
