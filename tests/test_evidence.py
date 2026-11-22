"""Done evidence tests: contract checks run before the report is
final, passing means done, failing means unverified."""

import pytest
from fastapi.testclient import TestClient

from botropolis.core.evidence import run_check, run_checks
from botropolis.core.orchestrator import CEO
from botropolis.core.registry import AgentRegistry
from botropolis.core.schemas import Contract, ContractCheck
from botropolis.server import app

client = TestClient(app)

REQUEST = "Research the history of the abacus"


@pytest.fixture(scope="module")
def ceo():
    return CEO(AgentRegistry())


def contract_with(*checks):
    return Contract(goal="Write the history", checks=list(checks))


def test_passing_check_marks_passed():
    result = run_check(ContractCheck(command="true"))
    assert result.passed is True
    assert result.exit_code == 0
    assert result.expected_exit_code == 0


def test_failing_check_marks_failed():
    result = run_check(ContractCheck(command="false"))
    assert result.passed is False
    assert result.exit_code == 1


def test_expected_exit_code_is_respected():
    result = run_check(ContractCheck(command="false", expected_exit_code=1))
    assert result.passed is True
    assert result.exit_code == 1


def test_output_tail_is_kept_as_evidence():
    result = run_check(ContractCheck(command="echo hello-evidence"))
    assert result.passed is True
    assert "hello-evidence" in result.output_tail


def test_file_probe_check(tmp_path, monkeypatch):
    monkeypatch.setenv("BOTROPOLIS_WORKSPACE", str(tmp_path))
    missing = run_check(ContractCheck(command="test -f probe.txt"))
    assert missing.passed is False
    (tmp_path / "probe.txt").write_text("here", encoding="utf-8")
    present = run_check(ContractCheck(command="test -f probe.txt"))
    assert present.passed is True


def test_blocked_command_fails_without_running():
    result = run_check(ContractCheck(command="rm -rf /"))
    assert result.passed is False
    assert result.exit_code is None
    assert "refused" in result.output_tail


def test_run_checks_runs_all_in_order():
    results = run_checks(
        [ContractCheck(command="true"), ContractCheck(command="false")]
    )
    assert [r.passed for r in results] == [True, False]


def test_passing_checks_leave_report_done(ceo):
    report = ceo.handle(REQUEST, contract=contract_with(ContractCheck(command="true")))
    assert report.status == "done"
    assert len(report.evidence) == 1
    assert report.evidence[0].passed is True
    assert report.to_dict()["status"] == "done"


def test_failing_check_marks_report_unverified(ceo):
    contract = contract_with(
        ContractCheck(command="true"),
        ContractCheck(command="echo not-ready-yet && false", name="readiness"),
    )
    report = ceo.handle(REQUEST, contract=contract)
    assert report.status == "unverified"
    data = report.to_dict()
    assert data["status"] == "unverified"
    assert len(data["evidence"]) == 2
    failed = [item for item in data["evidence"] if not item["passed"]]
    assert len(failed) == 1
    assert failed[0]["exit_code"] == 1
    assert "not-ready-yet" in failed[0]["output_tail"]


def test_contract_without_checks_is_unchanged(ceo):
    report = ceo.handle(REQUEST, contract=Contract(goal="Write the history"))
    assert report.status == "done"
    assert report.evidence == []


def test_no_contract_is_unchanged(ceo):
    report = ceo.handle(REQUEST)
    assert report.status == "done"
    assert report.evidence == []


def test_team_report_is_verified_too(ceo):
    contract = contract_with(ContractCheck(command="false"))
    report = ceo.team(
        "Design a small ETL and review it",
        ["pipelines", "trainer"],
        rounds=1,
        contract=contract,
    )
    assert report.status == "unverified"
    assert len(report.evidence) == 1


def test_checks_round_trip_through_from_dict():
    contract = Contract.from_dict(
        {
            "goal": "g",
            "checks": [
                {"command": "pytest -q", "expected_exit_code": 0, "name": "tests"}
            ],
        }
    )
    assert contract.checks == [
        ContractCheck(command="pytest -q", expected_exit_code=0, name="tests")
    ]
    assert Contract.from_dict(contract.to_dict()) == contract


def test_ask_endpoint_runs_checks():
    resp = client.post(
        "/ask",
        json={
            "request": REQUEST,
            "contract": {
                "goal": "Write the history",
                "checks": [{"command": "false"}],
            },
        },
    )
    assert resp.status_code == 200
    body = resp.json()
    assert body["status"] == "unverified"
    assert body["contract"]["checks"][0]["command"] == "false"
    assert body["evidence"][0]["passed"] is False
    assert body["evidence"][0]["exit_code"] == 1
