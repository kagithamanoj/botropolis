"""Tests for the scenario eval runner."""

from pathlib import Path

from botropolis.eval import runner
from botropolis.eval.runner import EvalResult, run_all, run_scenario


def _scenario(**overrides):
    base = {
        "name": "demo",
        "toolkit": ["calculator"],
        "task": "do math",
        "replies": ["FINAL: done"],
        "expect": {"output_contains": "done"},
    }
    base.update(overrides)
    return base


def test_passing_scenario():
    result = run_scenario(_scenario())
    assert isinstance(result, EvalResult)
    assert result.passed is True
    assert result.details == []


def test_failing_output_check_reports_detail():
    result = run_scenario(_scenario(expect={"output_contains": "nope"}))
    assert result.passed is False
    assert any("nope" in d for d in result.details)


def test_tool_call_mismatch_is_reported():
    scenario = _scenario(
        replies=[
            'ACTION: {"tool": "calculator", "args": {"expression": "1 + 1"}}',
            "FINAL: 2",
        ],
        expect={
            "tool_calls": [
                {"tool": "calculator", "args": {"expression": "2 + 2"}}
            ]
        },
    )
    result = run_scenario(scenario)
    assert result.passed is False
    assert any("args" in d for d in result.details)


def test_unknown_toolkit_does_not_crash_runner():
    result = run_scenario(_scenario(toolkit=["nope_not_a_tool"]))
    assert result.passed is False
    assert any("error" in d for d in result.details)


def test_missing_name_still_runs():
    scenario = _scenario()
    del scenario["name"]
    result = run_scenario(scenario)
    assert result.name == "<unnamed>"


def test_run_all_loads_bundled_scenarios():
    results = run_all(Path(runner.__file__).resolve().parent / "scenarios")
    assert len(results) >= 3
    assert all(r.passed for r in results), [
        (r.name, r.details) for r in results if not r.passed
    ]
