"""CI workflow tests: the nightly eval run stays scheduled and wired up,
so the scenario evals cannot go unrun without anyone noticing."""

from pathlib import Path

import yaml

ROOT = Path(__file__).resolve().parents[1]
WORKFLOW = ROOT / ".github" / "workflows" / "nightly-eval.yml"


def _load():
    return yaml.safe_load(WORKFLOW.read_text(encoding="utf-8"))


def _steps(workflow):
    jobs = workflow["jobs"]
    assert len(jobs) == 1, "nightly eval should be a single job"
    return next(iter(jobs.values()))["steps"]


def test_nightly_eval_workflow_exists_and_is_scheduled():
    assert WORKFLOW.exists()
    workflow = _load()
    # PyYAML reads the `on` key as boolean True (YAML 1.1).
    triggers = workflow.get("on", workflow.get(True))
    assert "schedule" in triggers, "nightly eval must run on a schedule"
    crons = [entry["cron"] for entry in triggers["schedule"]]
    assert crons, "schedule must name at least one cron time"
    assert "workflow_dispatch" in triggers, "nightly eval must be runnable by hand"


def test_nightly_eval_runs_the_scenario_runner():
    steps = _steps(_load())
    runs = [step.get("run", "") for step in steps]
    assert any(
        "python -m botropolis.eval.runner" in run for run in runs
    ), "nightly eval must run the scenario eval runner"


def test_nightly_eval_installs_the_deps_the_runner_needs():
    steps = _steps(_load())
    install = " ".join(step.get("run", "") for step in steps)
    for package in ("pyyaml", "fastapi", "sympy"):
        assert package in install, f"nightly eval install must include {package}"
