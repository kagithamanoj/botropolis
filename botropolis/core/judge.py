"""A separate judge for contracted runs: the builder/judge split.

The tool loop's verify pass is the same agent checking its own
work. The judge is different: after the machine evidence checks
pass, the CEO asks another agent, the Reviewer (Sofia), to judge
the final report against the contract's done_when and exclusions,
in one single-shot call.

The judge is a direct model call, not an agent run: it never goes
through the tool loop, never consults, and never recurses into
judging. A failed verdict marks the report "unverified" exactly
like a failed machine check, with the judge's reason attached as
evidence. A pass leaves the status as the machine checks left it.

Degradation follows the verify pass: if the judge call errors, if
there is no reviewer agent, or if the reply cannot be parsed, the
judge is recorded as skipped and the machine-check result stands.
The judge never breaks a run.
"""

from __future__ import annotations

from typing import Any, Dict, Optional, Tuple

# The judge is the Reviewer: the agent whose whole job is reading
# work skeptically before it ships.
JUDGE_AGENT_NAME = "reviewer"

# The judge reads the summary plus per-agent outputs, capped so the
# prompt stays small.
REPORT_TEXT_CHARS = 4000


def judge_prompt(contract, report_text: str) -> str:
    """The judging message: contract terms, the report, what to
    return. The judge is told plainly it is not the builder."""
    lines = [
        "You are the judge for this run, not the builder. A different "
        "agent did the work below; your job is to say whether it met "
        "the contract it was given.",
        "",
        f"Contract goal: {contract.goal}",
    ]
    if contract.done_when:
        lines.append("Done when:")
        for item in contract.done_when:
            lines.append(f"- {item}")
    if contract.exclusions:
        lines.append("Out of scope, must not appear in the work:")
        for item in contract.exclusions:
            lines.append(f"- {item}")
    lines.append("")
    lines.append("Report to judge:")
    lines.append(report_text)
    lines.append("")
    lines.append(
        "Reply with exactly one of these. If the report meets every "
        "done-when item and stays out of scope, reply: VERDICT: PASS"
    )
    lines.append(
        "If it misses a done-when item or does something out of "
        "scope, reply: VERDICT: FAIL followed by one line saying "
        "what was missed or violated."
    )
    return "\n".join(lines)


def parse_verdict(text: str) -> Optional[Tuple[bool, str]]:
    """Read a judge reply. (passed, reason) or None when the reply
    cannot be parsed. A pass carries no reason. A fail's reason is
    the rest of the verdict line, or the next non-empty line when
    the verdict line carries none."""
    if not text:
        return None
    lines = [line.strip() for line in text.strip().splitlines()]
    for index, stripped in enumerate(lines):
        if not stripped.upper().startswith("VERDICT:"):
            continue
        rest = stripped[len("VERDICT:"):].strip()
        upper = rest.upper()
        if upper.startswith("PASS"):
            return True, ""
        if upper.startswith("FAIL"):
            reason = rest[len("FAIL"):].strip(" -:").strip()
            if not reason:
                following = [line for line in lines[index + 1:] if line]
                reason = following[0] if following else ""
            return False, reason or "no reason given"
    return None


def report_text(report) -> str:
    """The report as text for the judge: summary plus per-agent
    outputs, capped."""
    lines = []
    summary = (getattr(report, "summary", "") or "").strip()
    if summary:
        lines.append(summary)
    results = getattr(report, "results", None)
    if results is None:
        rounds = getattr(report, "rounds", []) or []
        results = [round_.result for round_ in rounds]
    for result in results or []:
        output = (result.output or "").strip()
        if output:
            lines.append(f"[{result.agent_name}] {output}")
    text = "\n\n".join(lines)
    if len(text) > REPORT_TEXT_CHARS:
        text = text[:REPORT_TEXT_CHARS] + "... [truncated]"
    return text


def run_judge(ceo, contract, report) -> Dict[str, Any]:
    """Ask the Reviewer to judge one report. Never raises.

    Returns {"verdict": "pass"|"fail"|"skipped", "reason": str}.
    """
    try:
        judge = ceo.registry.get(JUDGE_AGENT_NAME)
    except Exception:
        return {"verdict": "skipped", "reason": "no reviewer agent in the registry"}
    try:
        messages = [
            {
                "role": "system",
                "content": judge.system_prompt
                + "\n\nYou are judging this run, not doing the work.",
            },
            {
                "role": "user",
                "content": judge_prompt(contract, report_text(report)),
            },
        ]
        reply = judge.client.chat(judge.model, messages)
        parsed = parse_verdict(reply)
        if parsed is None:
            return {
                "verdict": "skipped",
                "reason": "judge reply could not be parsed",
            }
        passed, reason = parsed
        return {
            "verdict": "pass" if passed else "fail",
            "reason": reason,
        }
    except Exception as exc:
        return {"verdict": "skipped", "reason": f"judge call failed: {exc}"}
