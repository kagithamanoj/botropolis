"""Sandbox helpers for agent tools.

Every file and shell tool runs inside a workspace directory and can never
touch anything outside it. The workspace defaults to
botropolis/data/workspace (created on first use, gitignored) and can be
moved with the BOTROPOLIS_WORKSPACE environment variable.

The shell also refuses a denylist of destructive commands. The denylist
is a guardrail against accidents, not a security boundary: treat tool
access like giving a junior engineer a terminal on a scratch machine.
"""

from __future__ import annotations

import os
import re
from pathlib import Path
from typing import List, Tuple

# (pattern, reason) pairs. Checked case-insensitively against the raw
# command string before anything runs.
_BLOCKED_COMMANDS: List[Tuple[str, str]] = [
    (r"\brm\b[^;|&]*\s/(?:\s|$|;)", "deleting from filesystem root"),
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
]


def check_shell_command(command: str) -> None:
    """Raise ValueError if the command matches the destructive denylist."""
    for pattern, reason in _BLOCKED_COMMANDS:
        if re.search(pattern, command, re.IGNORECASE):
            raise ValueError(f"Blocked command ({reason}): {command[:120]}")


def workspace_root() -> Path:
    """Return the workspace directory, creating it on first use."""
    override = os.environ.get("BOTROPOLIS_WORKSPACE")
    if override:
        root = Path(override).expanduser().resolve()
    else:
        root = Path(__file__).resolve().parents[1] / "data" / "workspace"
    root.mkdir(parents=True, exist_ok=True)
    return root


def resolve_workspace_path(path: str) -> Path:
    """Resolve a tool path inside the workspace.

    Raises ValueError when the path escapes the workspace (absolute
    paths outside it, or .. segments that climb above it).
    """
    root = workspace_root()
    target = (root / path).resolve()
    if target != root and root not in target.parents:
        raise ValueError(f"Path escapes the workspace: {path}")
    return target


def workspace_relative(path: Path) -> str:
    """Render a workspace path relative to the workspace root for display."""
    try:
        return str(path.relative_to(workspace_root()))
    except ValueError:
        return str(path)
