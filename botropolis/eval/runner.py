"""Scenario evals for Botropolis agents.

Each scenario is a YAML file describing a scripted model conversation:
the replies the model would give, and what the agent should do with
them. The runner plays the replies through a real Agent and checks the
outcome, so tool-use behavior gets regression coverage without live
model calls.

Scenario shape:

    name: calculator_two_step
    description: Agent multiplies with the calculator, then answers.
    toolkit: [calculator]
    task: What is 6 times 7?
    replies:
      - 'ACTION: {"tool": "calculator", "args": {"expression": "6 * 7"}}'
      - "FINAL: 42"
    expect:
      output_contains: "42"
      tool_calls:
        - tool: calculator
          args: {expression: "6 * 7"}

Run all scenarios with: python -m botropolis.eval.runner
"""

from __future__ import annotations

import sys
import tempfile
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Dict, List, Optional

import yaml

from botropolis.core.agent import Agent
from botropolis.core.schemas import Task

SCENARIOS_DIR = Path(__file__).resolve().parent / "scenarios"


class ScriptedClient:
    """ModelClient stand-in that plays canned replies in order."""

    def __init__(self, replies: List[str]):
        self.replies = list(replies)
        self.calls = 0

    def chat(self, model_id: str, messages: List[Dict[str, str]]) -> str:
        self.calls += 1
        if self.calls <= len(self.replies):
            return self.replies[self.calls - 1]
        return self.replies[-1]

    def provider_for(self, model_id: str) -> str:
        return "scripted"


@dataclass
class EvalResult:
    name: str
    passed: bool
    details: List[str] = field(default_factory=list)


def load_scenarios(directory: Path = SCENARIOS_DIR) -> List[Dict[str, Any]]:
    """Load every *.yaml scenario from a directory, sorted by name."""
    scenarios = []
    for path in sorted(directory.glob("*.yaml")):
        with open(path, "r", encoding="utf-8") as fh:
            scenarios.append(yaml.safe_load(fh) or {})
    return scenarios


def _check_tool_calls(
    expected: List[Dict[str, Any]], actual: List[Any]
) -> List[str]:
    """Compare expected tool calls against the real ones, in order."""
    problems = []
    if len(actual) != len(expected):
        problems.append(
            f"expected {len(expected)} tool calls, got {len(actual)}"
        )
        return problems
    for i, (want, got) in enumerate(zip(expected, actual)):
        if got.tool != want.get("tool"):
            problems.append(
                f"call {i}: expected tool {want.get('tool')!r}, got {got.tool!r}"
            )
        want_args = want.get("args") or {}
        if got.args != want_args:
            problems.append(
                f"call {i}: expected args {want_args}, got {got.args}"
            )
        if want.get("success") is not None and got.success != want["success"]:
            problems.append(
                f"call {i}: expected success={want['success']}, got {got.success}"
            )
    return problems


def run_scenario(scenario: Dict[str, Any]) -> EvalResult:
    """Run one scenario dict. Never raises; failures become details."""
    name = scenario.get("name", "<unnamed>")
    details: List[str] = []
    try:
        spec = {
            "name": "EvalAgent",
            "title": "Eval agent",
            "department": "research",
            "specialty": "scenario evals",
            "model": "stub",
            "system_prompt": "You are an eval agent.",
            "toolkit": scenario.get("toolkit", []),
        }
        with tempfile.NamedTemporaryFile(
            "w", suffix=".yaml", delete=False, encoding="utf-8"
        ) as fh:
            yaml.safe_dump(spec, fh)
            spec_path = Path(fh.name)
        agent = Agent(spec_path, client=ScriptedClient(scenario.get("replies", [])))
        result = agent.run(
            Task(
                description=scenario.get("task", ""),
                department="research",
                agent_name="EvalAgent",
            )
        )
    except Exception as exc:  # noqa: BLE001 - evals must not crash the runner
        return EvalResult(name=name, passed=False, details=[f"error: {exc}"])

    expect = scenario.get("expect", {}) or {}
    if "output_contains" in expect:
        if expect["output_contains"] not in (result.output or ""):
            details.append(
                f"output does not contain {expect['output_contains']!r}"
            )
    if "output_equals" in expect:
        if (result.output or "").strip() != str(expect["output_equals"]).strip():
            details.append(
                f"output {result.output!r} != {expect['output_equals']!r}"
            )
    if "tool_call_count" in expect:
        if len(result.tool_calls) != expect["tool_call_count"]:
            details.append(
                f"expected {expect['tool_call_count']} tool calls, "
                f"got {len(result.tool_calls)}"
            )
    if "tool_calls" in expect:
        details.extend(_check_tool_calls(expect["tool_calls"], result.tool_calls))
    if expect.get("success") is not None:
        if result.success != expect["success"]:
            details.append(f"expected success={expect['success']}, got {result.success}")

    return EvalResult(name=name, passed=not details, details=details)


def run_all(directory: Path = SCENARIOS_DIR) -> List[EvalResult]:
    """Run every scenario in a directory."""
    return [run_scenario(s) for s in load_scenarios(directory)]


def main(argv: Optional[List[str]] = None) -> int:
    directory = Path(argv[0]) if argv else SCENARIOS_DIR
    results = run_all(directory)
    passed = sum(1 for r in results if r.passed)
    for result in results:
        status = "PASS" if result.passed else "FAIL"
        print(f"[{status}] {result.name}")
        for detail in result.details:
            print(f"       - {detail}")
    print(f"\n{passed}/{len(results)} scenarios passed")
    return 0 if passed == len(results) else 1


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
