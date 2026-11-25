//! Done evidence: the harness verifies what the agent claims.
//!
//! A contract can carry machine checks (ContractCheck): shell commands
//! with an expected exit code. Before the CEO finalizes a report for a
//! contracted run, the checks run here and their results are attached
//! to the report as evidence. This is the builder/judge split: agents
//! produce the work and say it is done, the harness checks the claim.
//!
//! Checks run inside the agent sandbox: same workspace, same
//! destructive-command denylist as the shell tool. A refused command is
//! a failed check, never an executed one. A report whose checks all
//! pass is "done"; any failure makes it "unverified". Mirrors
//! `botropolis/core/evidence.py`.

use crate::sandbox::{check_shell_command, workspace_root};
use crate::schemas::{CheckResult, ContractCheck};
use std::time::{Duration, Instant};

/// How much of a check's combined output is kept as evidence.
pub const OUTPUT_TAIL_CHARS: usize = 500;

/// Checks are probes, not builds. A minute is generous.
pub const CHECK_TIMEOUT_SECONDS: u64 = 60;

/// Run one contract check and return its evidence. Never raises.
pub fn run_check(check: &ContractCheck) -> CheckResult {
    let base_command = check.command.clone();
    let base_expected = check.expected_exit_code;
    if let Err(e) = check_shell_command(&check.command) {
        return CheckResult {
            command: base_command,
            expected_exit_code: base_expected,
            exit_code: None,
            passed: false,
            output_tail: format!("refused by the sandbox: {e}"),
        };
    }
    let mut child = match std::process::Command::new("sh")
        .arg("-c")
        .arg(&check.command)
        .current_dir(workspace_root())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(e) => {
            return CheckResult {
                command: base_command,
                expected_exit_code: base_expected,
                exit_code: None,
                passed: false,
                output_tail: format!("could not run: {e}"),
            };
        }
    };
    let deadline = Instant::now() + Duration::from_secs(CHECK_TIMEOUT_SECONDS);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let output = child
                    .wait_with_output()
                    .unwrap_or_else(|_| std::process::Output {
                        status,
                        stdout: Vec::new(),
                        stderr: Vec::new(),
                    });
                let mut combined = String::from_utf8_lossy(&output.stdout).to_string();
                combined.push_str(&String::from_utf8_lossy(&output.stderr));
                let tail = tail_chars(&combined, OUTPUT_TAIL_CHARS);
                let code = status.code().unwrap_or(-1);
                return CheckResult {
                    command: base_command,
                    expected_exit_code: base_expected,
                    exit_code: Some(code),
                    passed: code == base_expected,
                    output_tail: tail,
                };
            }
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return CheckResult {
                        command: base_command,
                        expected_exit_code: base_expected,
                        exit_code: None,
                        passed: false,
                        output_tail: format!("timed out after {CHECK_TIMEOUT_SECONDS}s"),
                    };
                }
                std::thread::sleep(Duration::from_millis(25));
            }
            Err(e) => {
                return CheckResult {
                    command: base_command,
                    expected_exit_code: base_expected,
                    exit_code: None,
                    passed: false,
                    output_tail: format!("could not run: {e}"),
                };
            }
        }
    }
}

/// Run every check on a contract, in order.
pub fn run_checks(checks: &[ContractCheck]) -> Vec<CheckResult> {
    checks.iter().map(run_check).collect()
}

fn tail_chars(text: &str, limit: usize) -> String {
    let count = text.chars().count();
    if count <= limit {
        text.to_string()
    } else {
        text.chars().skip(count - limit).collect()
    }
}
