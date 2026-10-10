"""Typed output contracts for Botropolis agents.

Every agent declares the shape of its answer. The declaration lives in
the agent's YAML spec as `output_schema`, naming one of the registered
schemas below; when the spec omits it, a sensible default is picked per
department. The framework asks the model for JSON matching that schema,
validates it with Pydantic, and retries once with the validation errors
spelled out when it does not match. Hand-offs between agents are
therefore type-checked instead of stringly-typed.

Design rule: every schema requires only `summary`. Everything else is
optional, so a terse but valid answer never fails validation for being
short.
"""

from __future__ import annotations

import json
from typing import Any, Dict, List, Type

from pydantic import BaseModel, Field, ValidationError


class AgentOutput(BaseModel):
    """Default contract every agent satisfies.

    summary: the hand-off. What the CEO and the next agent rely on
    without reading the rest.
    details: the full answer in prose.
    key_points: the checkable facts, one per item.
    """

    summary: str = Field(min_length=1)
    details: str = ""
    key_points: List[str] = Field(default_factory=list)


class ResearchOutput(AgentOutput):
    """Research answers: findings plus where they came from."""

    sources: List[str] = Field(default_factory=list)
    open_questions: List[str] = Field(default_factory=list)


class CodeOutput(AgentOutput):
    """Code answers: what changed and whether tests agree."""

    files_changed: List[str] = Field(default_factory=list)
    tests_passed: bool = False


class RecommendationOutput(AgentOutput):
    """Advisory answers: risks spelled out and a clear recommendation."""

    risks: List[str] = Field(default_factory=list)
    recommendation: str = ""


OUTPUT_SCHEMAS: Dict[str, Type[AgentOutput]] = {
    "agent_output": AgentOutput,
    "research_output": ResearchOutput,
    "code_output": CodeOutput,
    "recommendation_output": RecommendationOutput,
}

# Sensible default per department. A spec's `output_schema` field
# overrides this; unknown names fail fast at agent load time.
DEPARTMENT_OUTPUT_SCHEMA: Dict[str, str] = {
    "research": "research_output",
    "code": "code_output",
    "finance": "recommendation_output",
    "legal": "recommendation_output",
    "health": "recommendation_output",
    "security": "recommendation_output",
    "data": "recommendation_output",
    "marketing": "agent_output",
    "ops": "agent_output",
    "support": "agent_output",
}

DEFAULT_OUTPUT_SCHEMA = "agent_output"


def resolve_output_schema(
    name: str | None, department: str
) -> tuple[str, Type[AgentOutput]]:
    """Resolve a schema name to (name, model class).

    Precedence: explicit spec value, then the department default, then
    the generic default. Raises ValueError on unknown names so a typo
    in a spec fails at load time, not at 2 AM in production.
    """
    wanted = name or DEPARTMENT_OUTPUT_SCHEMA.get(department, DEFAULT_OUTPUT_SCHEMA)
    try:
        return wanted, OUTPUT_SCHEMAS[wanted]
    except KeyError:
        raise ValueError(
            f"unknown output_schema '{wanted}'. Available: {sorted(OUTPUT_SCHEMAS)}"
        )


class OutputValidationError(ValueError):
    """Raised when model output does not match the declared schema."""


def extract_json(raw: str) -> str:
    """Pull the JSON payload out of raw model text.

    Handles ```json fences, bare ``` fences, and plain JSON. Returns
    the candidate JSON string, still unparsed.
    """
    text = raw.strip()
    if text.startswith("```"):
        lines = text.splitlines()[1:]  # drop the opening fence
        if lines and lines[-1].strip().startswith("```"):
            lines = lines[:-1]  # drop the closing fence
        text = "\n".join(lines).strip()
    return text


def _summarize_errors(exc: ValidationError) -> str:
    """Render Pydantic errors as one short, model-readable line."""
    parts = []
    for err in exc.errors():
        loc = ".".join(str(p) for p in err["loc"]) or "(root)"
        parts.append(f"{loc}: {err['msg']}")
    return "; ".join(parts)


def parse_structured_output(raw: str, schema: Type[AgentOutput]) -> AgentOutput:
    """Parse raw model text into a validated schema instance.

    Raises OutputValidationError with a human-readable reason when the
    text is not JSON or does not satisfy the schema. Every error names
    the schema so logs and repair prompts say what was expected.
    """
    name = schema.__name__
    try:
        data = json.loads(extract_json(raw))
    except json.JSONDecodeError as exc:
        raise OutputValidationError(f"output is not valid JSON for {name}: {exc}")
    if not isinstance(data, dict):
        raise OutputValidationError(
            f"output JSON for {name} must be an object, not a list or value"
        )
    try:
        return schema.model_validate(data)
    except ValidationError as exc:
        raise OutputValidationError(
            f"output does not match the {name} schema: {_summarize_errors(exc)}"
        )


def format_instruction(schema: Type[AgentOutput]) -> str:
    """Build the prompt instruction that asks for schema-shaped JSON."""
    compact = json.dumps(schema.model_json_schema(), separators=(",", ":"))
    return (
        "Your final answer must be a single JSON object matching this schema:\n"
        f"{compact}\n"
        "Reply with ONLY the JSON object. No prose, no fences, no explanation around it."
    )


def validated_dump(model: AgentOutput) -> Dict[str, Any]:
    """Return the validated model as a plain JSON-serializable dict."""
    return model.model_dump(mode="json")
