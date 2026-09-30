"""Usage analytics for Botropolis.

Every agent invocation (through the CEO or direct) is appended as one
JSON line to a local log. Nothing leaves the machine; it is just a
record of who ran, how long it took, and what the model reported.

The log lives at botropolis/data/usage.jsonl and is gitignored. Tests
point LOG_PATH at a temp file instead.
"""

from __future__ import annotations

import json
import time
from collections import defaultdict
from pathlib import Path
from typing import Any, Dict

LOG_PATH = Path(__file__).resolve().parent.parent / "data" / "usage.jsonl"


def log_invocation(
    agent_name: str,
    department: str,
    model: str,
    provider: str,
    latency_ms: float,
    tokens_in: int = 0,
    tokens_out: int = 0,
    success: bool = True,
) -> Dict[str, Any]:
    """Append one agent invocation to the usage log.

    Never raises: analytics must not break the request path.
    """
    row = {
        "ts": time.time(),
        "agent": agent_name,
        "department": department,
        "model": model,
        "provider": provider,
        "latency_ms": round(latency_ms, 2),
        "tokens_in": int(tokens_in),
        "tokens_out": int(tokens_out),
        "success": bool(success),
    }
    try:
        LOG_PATH.parent.mkdir(parents=True, exist_ok=True)
        with open(LOG_PATH, "a", encoding="utf-8") as fh:
            fh.write(json.dumps(row) + "\n")
    except OSError:
        pass
    return row


def get_summary() -> Dict[str, Any]:
    """Aggregate the usage log into per-agent totals.

    Returns {"agents": {name: {...}}, "totals": {...}}. Reads never raise;
    a missing log just yields empty totals.
    """
    calls = 0
    successes = 0
    stub_calls = 0
    per_agent: Dict[str, Dict[str, Any]] = defaultdict(
        lambda: {
            "calls": 0,
            "successes": 0,
            "latency_ms_total": 0.0,
            "tokens_in": 0,
            "tokens_out": 0,
            "stub_calls": 0,
            "models": set(),
        }
    )
    try:
        with open(LOG_PATH, "r", encoding="utf-8") as fh:
            for line in fh:
                line = line.strip()
                if not line:
                    continue
                try:
                    row = json.loads(line)
                except json.JSONDecodeError:
                    continue
                name = row.get("agent", "?")
                entry = per_agent[name]
                entry["calls"] += 1
                entry["latency_ms_total"] += float(row.get("latency_ms", 0) or 0)
                entry["tokens_in"] += int(row.get("tokens_in", 0) or 0)
                entry["tokens_out"] += int(row.get("tokens_out", 0) or 0)
                if row.get("success"):
                    entry["successes"] += 1
                    successes += 1
                if row.get("provider") == "stub":
                    entry["stub_calls"] += 1
                    stub_calls += 1
                if row.get("model"):
                    entry["models"].add(row["model"])
                calls += 1
    except OSError:
        pass

    agents: Dict[str, Dict[str, Any]] = {}
    for name, entry in sorted(per_agent.items()):
        n = entry["calls"]
        agents[name] = {
            "calls": n,
            "successes": entry["successes"],
            "avg_latency_ms": round(entry["latency_ms_total"] / n, 2) if n else 0.0,
            "tokens_in": entry["tokens_in"],
            "tokens_out": entry["tokens_out"],
            "tokens_total": entry["tokens_in"] + entry["tokens_out"],
            "stub_calls": entry["stub_calls"],
            "models": sorted(entry["models"]),
        }
    return {
        "agents": agents,
        "totals": {
            "calls": calls,
            "successes": successes,
            "stub_calls": stub_calls,
            "log_path": str(LOG_PATH),
        },
    }
