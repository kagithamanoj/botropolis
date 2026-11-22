"""Contract tests: the agreement reaches the prompt, rides on the
report, and lands in the ledger."""

import pytest
from fastapi.testclient import TestClient

from botropolis.core.ledger import Ledger
from botropolis.core.orchestrator import CEO
from botropolis.core.registry import AgentRegistry
from botropolis.core.schemas import Contract, Task
from botropolis.server import app

client = TestClient(app)


@pytest.fixture()
def contract():
    return Contract(
        goal="Write a short history of the abacus",
        scope=["origins in Mesopotamia", "spread to China"],
        exclusions=["modern calculators", "buying links"],
        done_when=["covers the origins", "under 500 words"],
    )


@pytest.fixture(scope="module")
def ceo():
    return CEO(AgentRegistry())


def test_contract_reaches_the_agent_prompt(contract):
    registry = AgentRegistry()
    agent = registry.get("pipelines")
    task = Task(
        description="Write the history",
        department="data",
        agent_name="pipelines",
        contract=contract,
    )
    user_message = agent.build_messages(task)[1]["content"]
    assert "Contract for this task:" in user_message
    assert "goal: Write a short history of the abacus" in user_message
    assert "origins in Mesopotamia" in user_message
    assert "done when: covers the origins" in user_message


def test_exclusions_are_stated_as_out_of_scope(contract):
    registry = AgentRegistry()
    agent = registry.get("pipelines")
    task = Task(
        description="Write the history",
        department="data",
        agent_name="pipelines",
        contract=contract,
    )
    user_message = agent.build_messages(task)[1]["content"]
    assert "out of scope, do not do these: modern calculators" in user_message


def test_no_contract_means_no_contract_section():
    registry = AgentRegistry()
    agent = registry.get("pipelines")
    task = Task(description="Write the history", department="data")
    user_message = agent.build_messages(task)[1]["content"]
    assert "Contract for this task:" not in user_message


def test_plan_attaches_contract_to_every_task(ceo, contract):
    tasks = ceo.plan("Research the history of the abacus", contract=contract)
    assert tasks
    assert all(task.contract is contract for task in tasks)


def test_handle_report_carries_the_contract(ceo, contract):
    report = ceo.handle(
        "Research the history of the abacus", contract=contract
    )
    assert report.contract is contract
    data = report.to_dict()
    assert data["contract"]["goal"] == "Write a short history of the abacus"
    assert data["contract"]["exclusions"] == ["modern calculators", "buying links"]
    assert data["contract"]["done_when"] == ["covers the origins", "under 500 words"]


def test_team_report_carries_the_contract(ceo, contract):
    report = ceo.team(
        "Design a small ETL and review it",
        ["pipelines", "trainer"],
        rounds=1,
        contract=contract,
    )
    assert report.contract is contract
    assert report.to_dict()["contract"]["goal"] == contract.goal


def test_handle_without_contract_reports_none(ceo):
    report = ceo.handle("Research the history of the abacus")
    assert report.contract is None
    assert report.to_dict()["contract"] is None


def test_ledger_records_the_contract(contract):
    ledger = Ledger()
    ledger_ceo = CEO(AgentRegistry(), ledger=ledger)
    ledger_ceo.handle("Research the history of the abacus", contract=contract)
    run_id = ledger.runs()[0]["run_id"]
    started = [
        event for event in ledger.events(run_id) if event["kind"] == "run_started"
    ]
    assert started
    assert started[0]["payload"]["contract"]["goal"] == contract.goal
    assert started[0]["payload"]["contract"]["scope"] == contract.scope


def test_ledger_without_contract_has_no_contract_key():
    ledger = Ledger()
    ledger_ceo = CEO(AgentRegistry(), ledger=ledger)
    ledger_ceo.handle("Research the history of the abacus")
    run_id = ledger.runs()[0]["run_id"]
    started = [
        event for event in ledger.events(run_id) if event["kind"] == "run_started"
    ]
    assert "contract" not in started[0]["payload"]


def test_contract_from_dict_round_trip(contract):
    rebuilt = Contract.from_dict(contract.to_dict())
    assert rebuilt == contract
    assert Contract.from_dict(None) is None
    assert Contract.from_dict({}) is None


def test_ask_endpoint_accepts_a_contract():
    resp = client.post(
        "/ask",
        json={
            "request": "Research the history of the abacus",
            "contract": {
                "goal": "Write a short history of the abacus",
                "scope": ["origins in Mesopotamia"],
                "exclusions": ["modern calculators"],
                "done_when": ["covers the origins"],
            },
        },
    )
    assert resp.status_code == 200
    body = resp.json()
    assert body["contract"]["goal"] == "Write a short history of the abacus"
    assert body["contract"]["exclusions"] == ["modern calculators"]


def test_ask_endpoint_without_contract_still_works():
    resp = client.post("/ask", json={"request": "Research the history of the abacus"})
    assert resp.status_code == 200
    assert resp.json()["contract"] is None
