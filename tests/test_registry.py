"""Registry tests: every spec loads and the company directory is complete."""

import pytest

from botropolis.core.registry import AgentRegistry

EXPECTED_DEPARTMENTS = {
    "research",
    "health",
    "finance",
    "code",
    "data",
    "legal",
    "marketing",
    "ops",
    "security",
    "support",
}

EXPECTED_COUNTS = {
    "research": 3,
    "health": 3,
    "finance": 2,
    "code": 3,
    "data": 2,
    "legal": 1,
    "marketing": 2,
    "ops": 2,
    "security": 1,
    "support": 1,
}


@pytest.fixture(scope="module")
def registry():
    return AgentRegistry()


def test_all_twenty_specs_load(registry):
    assert len(registry) == 20


def test_departments_match(registry):
    assert set(registry.departments) == EXPECTED_DEPARTMENTS


def test_department_headcounts(registry):
    for dept, count in EXPECTED_COUNTS.items():
        assert len(registry.by_department(dept)) == count, dept


def test_required_spec_fields(registry):
    for agent in registry.list_all():
        assert agent.name
        assert agent.title
        assert agent.department
        assert agent.specialty
        assert agent.model
        assert len(agent.system_prompt.split()) > 30  # real prompt, not a stub
        assert agent.example_tasks


def test_get_is_case_insensitive(registry):
    assert registry.get("scout").name == "Scout"
    assert registry.get("SCOUT").name == "Scout"


def test_get_unknown_raises(registry):
    with pytest.raises(KeyError):
        registry.get("NoSuchAgent")


def test_triagebot_has_medical_disclaimer(registry):
    prompt = registry.get("TriageBot").system_prompt.lower()
    assert "not a doctor" in prompt
    assert "medical advice" in prompt or "not medical advice" in prompt


def test_paralegal_has_legal_disclaimer(registry):
    prompt = registry.get("Paralegal").system_prompt.lower()
    assert "not a lawyer" in prompt
    assert "not legal advice" in prompt or "legal advice" in prompt
