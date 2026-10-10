"""Output contract tests: typed agent answers, validated with Pydantic."""

import json

import pytest
import yaml

from botropolis.core.agent import Agent
from botropolis.core.output_schemas import (
    OUTPUT_SCHEMAS,
    AgentOutput,
    OutputValidationError,
    ResearchOutput,
    parse_structured_output,
    resolve_output_schema,
)


class FakeClient:
    """Scripted stand-in for ModelClient with a configurable provider."""

    def __init__(self, replies, provider="openai"):
        self.replies = list(replies)
        self.provider = provider
        self.calls = 0
        self.seen_messages = []
        self.last_usage = {"input": 0, "output": 0}

    def chat(self, model_id, messages):
        self.calls += 1
        self.seen_messages.append(messages)
        if self.calls <= len(self.replies):
            return self.replies[self.calls - 1]
        return self.replies[-1]

    def provider_for(self, model_id):
        return self.provider


def make_agent(tmp_path, replies, provider="openai", extra_spec=None):
    spec = {
        "name": "TestAgent",
        "title": "Tester",
        "department": "ops",
        "specialty": "testing",
        "model": "gpt-4o",
        "system_prompt": "You are a test agent.",
    }
    spec.update(extra_spec or {})
    path = tmp_path / "testagent.yaml"
    path.write_text(yaml.safe_dump(spec), encoding="utf-8")
    client = FakeClient(replies, provider=provider)
    agent = Agent(path, client=client)
    return agent, client


def good_reply(**overrides):
    payload = {"summary": "Did the thing.", "details": "All details here.",
               "key_points": ["one", "two"]}
    payload.update(overrides)
    return json.dumps(payload)


# ── schema resolution ─────────────────────────────────────────────────────

def test_known_schemas_resolve():
    name, cls = resolve_output_schema("research_output", "ops")
    assert name == "research_output"
    assert cls is ResearchOutput


def test_department_defaults_apply():
    assert resolve_output_schema(None, "research")[0] == "research_output"
    assert resolve_output_schema(None, "code")[0] == "code_output"
    assert resolve_output_schema(None, "finance")[0] == "recommendation_output"
    assert resolve_output_schema(None, "ops")[0] == "agent_output"
    assert resolve_output_schema(None, "brand_new_dept")[0] == "agent_output"


def test_explicit_spec_value_beats_department_default(tmp_path):
    agent, _ = make_agent(tmp_path, ["{}"], extra_spec={"output_schema": "code_output"})
    assert agent.output_schema_name == "code_output"


def test_unknown_schema_name_fails_at_load(tmp_path):
    with pytest.raises(ValueError, match="unknown output_schema"):
        make_agent(tmp_path, ["{}"], extra_spec={"output_schema": "teleporter"})


def test_every_registered_schema_needs_only_summary():
    for name, cls in OUTPUT_SCHEMAS.items():
        inst = cls(summary="x")
        assert inst.summary == "x", name


# ── parsing ───────────────────────────────────────────────────────────────

def test_parse_plain_json():
    model = parse_structured_output(good_reply(), AgentOutput)
    assert model.summary == "Did the thing."
    assert model.key_points == ["one", "two"]


def test_parse_fenced_json():
    fenced = "```json\n" + good_reply() + "\n```"
    model = parse_structured_output(fenced, AgentOutput)
    assert model.summary == "Did the thing."


def test_parse_rejects_non_json():
    with pytest.raises(OutputValidationError, match="not valid JSON"):
        parse_structured_output("just some prose", AgentOutput)


def test_parse_rejects_non_object_json():
    with pytest.raises(OutputValidationError, match="must be an object"):
        parse_structured_output("[1, 2, 3]", AgentOutput)


def test_parse_prose_wrapped_json():
    wrapped = "Here is my answer: " + good_reply() + " Hope that helps."
    model = parse_structured_output(wrapped, AgentOutput)
    assert model.summary == "Did the thing."


def test_parse_prefers_strict_candidate_first():
    # Valid JSON up front wins; the braced fallback is not needed.
    model = parse_structured_output(good_reply(), AgentOutput)
    assert model.summary == "Did the thing."


def test_parse_rejects_missing_summary():
    with pytest.raises(OutputValidationError, match="summary"):
        parse_structured_output(json.dumps({"details": "no summary"}), AgentOutput)


def test_parse_ignores_extra_fields():
    model = parse_structured_output(
        good_reply(extra_field="whatever"), AgentOutput
    )
    assert model.summary == "Did the thing."


# ── agent run: happy path ─────────────────────────────────────────────────

def test_valid_output_passes_and_renders(tmp_path):
    agent, client = make_agent(tmp_path, [good_reply()])
    result = agent.run("Do the thing")
    assert result.success
    assert client.calls == 1  # no retry needed
    assert result.output.startswith("Did the thing.\n\nAll details here.")
    assert result.structured["summary"] == "Did the thing."
    assert result.structured["key_points"] == ["one", "two"]
    assert result.metadata["output_schema"] == "agent_output"
    assert result.metadata["output_validated"] is True


def test_prompt_asks_for_json(tmp_path):
    agent, _ = make_agent(tmp_path, [good_reply()])
    messages = agent.build_messages(__import__("botropolis.core.schemas", fromlist=["Task"]).Task(description="x"))
    assert "JSON object" in messages[-1]["content"]
    assert "summary" in messages[-1]["content"]


def test_structured_survives_to_dict(tmp_path):
    agent, _ = make_agent(tmp_path, [good_reply()])
    result = agent.run("Do the thing")
    as_dict = result.to_dict()
    assert as_dict["structured"]["summary"] == "Did the thing."


# ── agent run: retry behavior ─────────────────────────────────────────────

def test_invalid_json_retries_once_then_fails(tmp_path):
    agent, client = make_agent(tmp_path, ["not json at all", "still not json"])
    result = agent.run("Do the thing")
    assert not result.success
    assert client.calls == 2  # exactly one repair attempt
    assert "not valid JSON" in result.error


def test_schema_violation_retries_with_errors_spelled_out(tmp_path):
    bad = json.dumps({"details": "missing the summary field"})
    agent, client = make_agent(tmp_path, [bad, good_reply()])
    result = agent.run("Do the thing")
    assert result.success
    assert client.calls == 2
    repair_prompt = client.seen_messages[1][-1]["content"]
    assert "summary" in repair_prompt  # the validation error is spelled out


def test_repair_failure_reports_schema_name(tmp_path):
    agent, _ = make_agent(
        tmp_path, ["oops", "oops again"], extra_spec={"output_schema": "code_output"}
    )
    result = agent.run("Do the thing")
    assert not result.success
    assert "CodeOutput" in result.error


# ── stub path ─────────────────────────────────────────────────────────────

def test_stub_skips_validation_and_wraps_prose(tmp_path):
    agent, client = make_agent(
        tmp_path, ["[offline stub] some prose answer"], provider="stub"
    )
    result = agent.run("Do the thing")
    assert result.success
    assert client.calls == 1  # no validation, no retry
    assert result.output == "[offline stub] some prose answer"
    assert result.structured["summary"] == "[offline stub] some prose answer"
    assert result.metadata["output_validated"] is False


# ── tool loop path ────────────────────────────────────────────────────────

def test_tool_loop_final_answer_is_validated(tmp_path):
    agent, client = make_agent(
        tmp_path,
        ['FINAL: {"summary": "Done via tools.", "details": "", "key_points": []}'],
        extra_spec={"toolkit": ["current_time"]},
    )
    result = agent.run("What time is it")
    assert result.success
    assert result.structured["summary"] == "Done via tools."
    assert result.metadata["output_validated"] is True
