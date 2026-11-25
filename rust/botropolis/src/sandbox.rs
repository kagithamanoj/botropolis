//! Sandbox helpers for agent tools.
//!
//! Every file and shell tool runs inside a workspace directory and can
//! never touch anything outside it. The workspace defaults to
//! `botropolis/data/workspace` (created on first use, gitignored) and
//! can be moved with the `BOTROPOLIS_WORKSPACE` environment variable.
//!
//! The shell also refuses a denylist of destructive commands. The
//! denylist is a guardrail against accidents, not a security boundary:
//! treat tool access like giving a junior engineer a terminal on a
//! scratch machine. Mirrors `botropolis/tools/sandbox.py`.

use regex::Regex;
use std::env;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// (pattern, reason) pairs. Checked case-insensitively against the raw
/// command string before anything runs.
fn blocked_commands() -> &'static Vec<(Regex, &'static str)> {
    static BLOCKED: OnceLock<Vec<(Regex, &'static str)>> = OnceLock::new();
    BLOCKED.get_or_init(|| {
        let pairs: &[(&str, &str)] = &[
            (
                r"\brm\b[^;|&]*\s/(?:\s|$|;)",
                "deleting from filesystem root",
            ),
            (r"\brm\s+-[a-z]*r", "recursive delete"),
            (r"\bmkfs\b", "filesystem formatting"),
            (r"\bdd\b.*\bof=/dev/", "raw disk write"),
            (r":\(\)\s*\{", "fork bomb"),
            (r"\bshutdown\b", "system shutdown"),
            (r"\breboot\b", "system reboot"),
            (r"\bhalt\b", "system halt"),
            (r"\bpoweroff\b", "system poweroff"),
            (r">\s*/dev/sd[a-z]", "raw disk write"),
            (r">\s*/dev/nvme", "raw disk write"),
            (r"\bchmod\s+-R\s+777\s+/", "permission change on root"),
        ];
        pairs
            .iter()
            .map(|(pattern, reason)| {
                (
                    Regex::new(&format!("(?i){pattern}")).expect("bad denylist pattern"),
                    *reason,
                )
            })
            .collect()
    })
}

/// Raise an error string when the command matches the destructive denylist.
pub fn check_shell_command(command: &str) -> Result<(), String> {
    for (pattern, reason) in blocked_commands() {
        if pattern.is_match(command) {
            let head: String = command.chars().take(120).collect();
            return Err(format!("Blocked command ({reason}): {head}"));
        }
    }
    Ok(())
}

/// Return the workspace directory, creating it on first use.
pub fn workspace_root() -> PathBuf {
    let root = match env::var("BOTROPOLIS_WORKSPACE") {
        Ok(override_path) if !override_path.is_empty() => PathBuf::from(expanduser(&override_path)),
        _ => crate::paths::data_dir().join("workspace"),
    };
    let _ = std::fs::create_dir_all(&root);
    root
}

fn expanduser(path: &str) -> String {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Ok(home) = env::var("HOME") {
            return format!("{home}/{rest}");
        }
    }
    path.to_string()
}

/// Resolve a tool path inside the workspace.
///
/// Returns an error when the path escapes the workspace (absolute
/// paths outside it, or `..` segments that climb above it).
pub fn resolve_workspace_path(path: &str) -> Result<PathBuf, String> {
    let root = workspace_root();
    let target = canonicalize_join(&root, path);
    if target != root && !target.starts_with(&root) {
        return Err(format!("Path escapes the workspace: {path}"));
    }
    Ok(target)
}

/// Join a path onto a root and resolve `.` and `..` lexically, so a
/// path that does not exist yet can still be checked.
fn canonicalize_join(root: &Path, path: &str) -> PathBuf {
    let mut out = root.to_path_buf();
    let candidate = Path::new(path);
    // An absolute tool path is taken as-is, then checked against the root.
    let parts = if candidate.is_absolute() {
        out = PathBuf::from("/");
        candidate.components()
    } else {
        candidate.components()
    };
    for component in parts {
        use std::path::Component::*;
        match component {
            RootDir => {}
            CurDir => {}
            ParentDir => {
                out.pop();
            }
            Normal(part) => out.push(part),
            Prefix(_) => {}
        }
    }
    out
}

/// Render a workspace path relative to the workspace root for display.
pub fn workspace_relative(path: &Path) -> String {
    match path.strip_prefix(workspace_root()) {
        Ok(rel) => rel.to_string_lossy().to_string(),
        Err(_) => path.to_string_lossy().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn denylist_blocks_destructive_commands() {
        assert!(check_shell_command("rm -rf /").is_err());
        assert!(check_shell_command("rm -rf /tmp/x").is_err());
        assert!(check_shell_command("echo hello").is_ok());
        assert!(check_shell_command("ls -la").is_ok());
        assert!(check_shell_command("dd if=/dev/zero of=/dev/sda").is_err());
        assert!(check_shell_command("shutdown -h now").is_err());
    }

    #[test]
    fn paths_cannot_escape_the_workspace() {
        let _guard = crate::test_support::lock_env();
        std::env::set_var("BOTROPOLIS_WORKSPACE", "/tmp/botropolis-ws-test");
        assert!(resolve_workspace_path("../secret.txt").is_err());
        assert!(resolve_workspace_path("/etc/passwd").is_err());
        assert!(resolve_workspace_path("notes/todo.txt").is_ok());
        std::env::remove_var("BOTROPOLIS_WORKSPACE");
    }
}
