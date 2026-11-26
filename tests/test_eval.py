"""Tests for the scenario eval runner."""

from pathlib import Path

from botropolis.eval import runner
from botropolis.eval.runner import EvalResult, run_all, run_scenario


def _scenario(**overrides):
    base = {
        "name": "demo",
        "toolkit": ["calculator"],
        "task": "do math",
        "replies": ['FINAL: {"summary": "done", "details": "", "key_points": []}'],
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
            'FINAL: {"summary": "2", "details": "", "key_points": []}',
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
    assert len(results) == 15
    names = {r.name for r in results}
    assert {
        "plan_first_then_act",
        "check_revises_answer",
        "same_error_escalates",
        "malformed_action_recovers",
        "approval_wall_blocks_apply",
    } <= names
    assert all(r.passed for r in results), [
        (r.name, r.details) for r in results if not r.passed
    ]


def test_scenario_env_setup_files_and_file_contains():
    scenario = _scenario(
        toolkit=["job_apply"],
        env={
            "BOTROPOLIS_WORKSPACE": "{tmp}/workspace",
            "BOTROPOLIS_APPLICATIONS_LOG": "{tmp}/applications.jsonl",
        },
        setup_files={"workspace/resume.txt": "Eval resume."},
        replies=[
            'ACTION: {"tool": "job_apply", "args": {"title": "Dev", "company": "Acme Corp", "url": "https://example.com/jobs/1", "resume": "resume.txt"}}',
            'FINAL: {"summary": "prepared", "details": "", "key_points": []}',
        ],
        expect={
            "output_contains": "prepared",
            "file_contains": [
                {"path": "{tmp}/applications.jsonl", "contains": "Acme Corp"}
            ],
        },
    )
    result = run_scenario(scenario)
    assert result.passed is True, result.details


def test_scenario_file_contains_missing_file_fails():
    scenario = _scenario(
        expect={
            "output_contains": "done",
            "file_contains": [{"path": "{tmp}/never_written.txt", "contains": "x"}],
        }
    )
    result = run_scenario(scenario)
    assert result.passed is False
    assert any("never_written" in d for d in result.details)


def test_scenario_needs_approval_blocks_tool_and_writes_nothing():
    scenario = _scenario(
        toolkit=["job_apply"],
        needs_approval=["job_apply"],
        env={
            "BOTROPOLIS_WORKSPACE": "{tmp}/workspace",
            "BOTROPOLIS_APPLICATIONS_LOG": "{tmp}/applications.jsonl",
        },
        setup_files={"workspace/resume.txt": "Eval resume."},
        replies=[
            'ACTION: {"tool": "job_apply", "args": {"title": "Dev", "company": "Acme Corp", "url": "https://example.com/jobs/1", "resume": "resume.txt"}}',
            'FINAL: {"summary": "blocked: job_apply needs human approval", "details": "", "key_points": []}',
        ],
        expect={
            "output_contains": "needs human approval",
            "tool_calls": [
                {
                    "tool": "job_apply",
                    "args": {
                        "title": "Dev",
                        "company": "Acme Corp",
                        "url": "https://example.com/jobs/1",
                        "resume": "resume.txt",
                    },
                    "success": False,
                }
            ],
            "file_absent": [{"path": "{tmp}/applications.jsonl"}],
        },
    )
    result = run_scenario(scenario)
    assert result.passed is True, result.details


def test_scenario_file_absent_fails_when_file_exists():
    scenario = _scenario(
        setup_files={"written.txt": "already here"},
        expect={
            "output_contains": "done",
            "file_absent": [{"path": "{tmp}/written.txt"}],
        },
    )
    result = run_scenario(scenario)
    assert result.passed is False
    assert any("absent" in d for d in result.details)


def test_scenario_same_error_twice_escalates():
    scenario = _scenario(
        replies=[
            'ACTION: {"tool": "calculator", "args": {"expression": "100 / (5 - 5)"}}',
            'ACTION: {"tool": "calculator", "args": {"expression": "100 / (5 - 5)"}}',
        ],
        expect={
            "output_contains": "failed twice in a row",
            "success": False,
            "tool_call_count": 2,
        },
    )
    result = run_scenario(scenario)
    assert result.passed is True, result.details


def test_scenario_check_pass_can_revise_the_answer():
    scenario = _scenario(
        toolkit=["calculator"],
        replies=[
            'FINAL: {"summary": "6 times 7 is 43.", "details": "", "key_points": []}',
            'FINAL: {"summary": "6 times 7 is 42.", "details": "", "key_points": []}',
        ],
        expect={"output_equals": "6 times 7 is 42."},
    )
    result = run_scenario(scenario)
    assert result.passed is True, result.details
