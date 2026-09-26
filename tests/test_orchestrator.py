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
