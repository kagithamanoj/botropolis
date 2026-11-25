"""Judge tests: a different agent judges contracted reports, and
the judge never breaks a run."""

import pytest

from botropolis.core.judge import judge_prompt, parse_verdict
from botropolis.core.orchestrator import CEO
from botropolis.core.registry import AgentRegistry
from botropolis.core.schemas import Contract, ContractCheck


class ScriptedClient:
    """Replies play in order; records the messages it was shown."""

    def __init__(self, replies):
        self.replies = list(replies)
        self.calls = 0
        self.last_usage = {"input": 0, "output": 0}
        self.seen = []

    def chat(self, model_id, messages):
        self.calls += 1
        self.seen.append([dict(m) for m in messages])
        if self.calls <= len(self.replies):
            return self.replies[self.calls - 1]
        return self.replies[-1]

    def provider_for(self, model_id):
        return "stub"


class RaisingClient:
    """Dies on every call, like a dead provider."""

    def __init__(self):
        self.calls = 0
        self.last_usage = {"input": 0, "output": 0}

    def chat(self, model_id, messages):
        self.calls += 1
        raise RuntimeError("provider down")

    def provider_for(self, model_id):
        return "stub"


@pytest.fixture()
def contract():
    return Contract(
        goal="Write a short history of the abacus",
        scope=["origins in Mesopotamia", "spread to China"],
        exclusions=["modern calculators", "buying links"],
        done_when=["covers the origins", "under 500 words"],
    )


def ceo_with_judge(client):
    ceo = CEO(AgentRegistry())
    ceo.registry.get("reviewer").client = client
    return ceo


def test_parse_verdict():
    assert parse_verdict("VERDICT: PASS") == (True, "")
    assert parse_verdict("verdict: pass") == (True, "")
    assert parse_verdict("VERDICT: FAIL\nThe report never covers the origins.") == (
        False,
        "The report never covers the origins.",
    )
    assert parse_verdict("VERDICT: FAIL") == (False, "no reason given")
    assert parse_verdict("Looks fine to me.") is None
    assert parse_verdict("") is None


def test_judge_prompt_names_terms(contract):
    text = judge_prompt(contract, "The abacus began in Mesopotamia.")
    assert "Write a short history of the abacus" in text
    assert "covers the origins" in text
    assert "modern calculators" in text
    assert "The abacus began in Mesopotamia." in text
    assert "not the builder" in text


def test_judge_pass_keeps_done(contract):
    client = ScriptedClient(["VERDICT: PASS"])
    ceo = ceo_with_judge(client)
    report = ceo.handle("Research the history of the abacus", contract=contract)
    assert report.status == "done"
    assert report.judge == {"verdict": "pass", "reason": ""}
    shown = "\n".join(
        m.get("content", "") for call in client.seen for m in call
    )
    assert "covers the origins" in shown
    assert "modern calculators" in shown


def test_judge_fail_flips_to_unverified(contract):
    client = ScriptedClient(
        ["VERDICT: FAIL\nThe report never covers the origins."]
    )
    ceo = ceo_with_judge(client)
    report = ceo.handle("Research the history of the abacus", contract=contract)
    assert report.status == "unverified"
    assert report.judge["verdict"] == "fail"
    assert report.judge["reason"] == "The report never covers the origins."
    judge_evidence = [item for item in report.evidence if item.command == "judge"]
    assert len(judge_evidence) == 1
    assert not judge_evidence[0].passed
    assert "never covers the origins" in judge_evidence[0].output_tail


def test_judge_error_is_skipped(contract):
    ceo = ceo_with_judge(RaisingClient())
    report = ceo.handle("Research the history of the abacus", contract=contract)
    assert report.status == "done"
    assert report.judge["verdict"] == "skipped"
    assert "judge call failed" in report.judge["reason"]


def test_judge_unparseable_is_skipped(contract):
    client = ScriptedClient(["Looks fine to me, ship it."])
    ceo = ceo_with_judge(client)
    report = ceo.handle("Research the history of the abacus", contract=contract)
    assert report.status == "done"
    assert report.judge["verdict"] == "skipped"
    assert "could not be parsed" in report.judge["reason"]


def test_no_contract_means_no_judge():
    client = ScriptedClient(["VERDICT: FAIL"])
    ceo = ceo_with_judge(client)
    report = ceo.handle("Research the history of the abacus")
    assert report.judge == {}
    assert client.calls == 0


def test_failed_machine_check_skips_judge(contract):
    contract.checks = [ContractCheck(command="false", expected_exit_code=0)]
    client = ScriptedClient(["VERDICT: PASS"])
    ceo = ceo_with_judge(client)
    report = ceo.handle("Research the history of the abacus", contract=contract)
    assert report.status == "unverified"
    assert report.judge["verdict"] == "skipped"
    assert report.judge["reason"] == "machine checks failed"
    assert client.calls == 0


def test_passing_machine_check_then_judge_passes(contract):
    contract.checks = [ContractCheck(command="true", expected_exit_code=0)]
    client = ScriptedClient(["VERDICT: PASS"])
    ceo = ceo_with_judge(client)
    report = ceo.handle("Research the history of the abacus", contract=contract)
    assert report.status == "done"
    assert report.judge["verdict"] == "pass"
    assert any(item.passed for item in report.evidence)


def test_team_report_is_judged(contract):
    client = ScriptedClient(
        ["VERDICT: FAIL\nThe team never covered the origins."]
    )
    ceo = ceo_with_judge(client)
    report = ceo.team(
        "Design a small ETL and review it",
        ["pipelines", "trainer"],
        rounds=1,
        contract=contract,
    )
    assert report.status == "unverified"
    assert report.judge["verdict"] == "fail"
    assert "never covered the origins" in report.judge["reason"]
