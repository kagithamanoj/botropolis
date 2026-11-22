"""Done evidence: the harness verifies what the agent claims.

A contract can carry machine checks (ContractCheck): shell commands
with an expected exit code. Before the CEO finalizes a report for a
contracted run, the checks run here and their results are attached
to the report as evidence. This is the builder/judge split: agents
produce the work and say it is done, the harness checks the claim.

Checks run inside the agent sandbox: same workspace, same
destructive-command denylist as the shell tool. A refused command is
a failed check, never an executed one. A report whose checks all
pass is "done"; any failure makes it "unverified".
"""

from __future__ import annotations

import subprocess
from typing import List

from botropolis.core.schemas import CheckResult, ContractCheck
from botropolis.tools.sandbox import check_shell_command, workspace_root

# How much of a check's combined output is kept as evidence.
OUTPUT_TAIL_CHARS = 500

# Checks are probes, not builds. A minute is generous.
CHECK_TIMEOUT_SECONDS = 60


def run_check(check: ContractCheck) -> CheckResult:
    """Run one contract check and return its evidence. Never raises."""
    base = {
        "command": check.command,
        "expected_exit_code": check.expected_exit_code,
    }
    try:
        check_shell_command(check.command)
    except ValueError as exc:
        return CheckResult(
            **base,
            exit_code=None,
            passed=False,
            output_tail=f"refused by the sandbox: {exc}",
        )
    try:
        proc = subprocess.run(
            check.command,
            shell=True,
            cwd=str(workspace_root()),
            capture_output=True,
            text=True,
            timeout=CHECK_TIMEOUT_SECONDS,
        )
    except subprocess.TimeoutExpired as exc:
        tail = (str(exc.stdout or "") + str(exc.stderr or ""))[-OUTPUT_TAIL_CHARS:]
        return CheckResult(
            **base,
            exit_code=None,
            passed=False,
            output_tail=f"timed out after {CHECK_TIMEOUT_SECONDS}s\n{tail}".strip(),
        )
    except OSError as exc:
        return CheckResult(
            **base, exit_code=None, passed=False, output_tail=f"could not run: {exc}"
        )
    output = (proc.stdout + proc.stderr)[-OUTPUT_TAIL_CHARS:]
    return CheckResult(
        **base,
        exit_code=proc.returncode,
        passed=proc.returncode == check.expected_exit_code,
        output_tail=output,
    )


def run_checks(checks: List[ContractCheck]) -> List[CheckResult]:
    """Run every check on a contract, in order."""
    return [run_check(check) for check in checks]
