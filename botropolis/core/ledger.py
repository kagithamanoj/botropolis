"""Append-only run ledger for Botropolis.

Every agent run appends events to a single ledger, SQLite-backed via
the standard library. The ledger is the durable trail of what happened:
tasks assigned, tools called, answers produced, contracts repaired,
runs finished. The CompanyReport remains the API's answer; the ledger
is the observable, replayable record underneath it, and the event
stream doubles as training and eval data.

The ledger is a side channel: recording never breaks a run. A failing
ledger is swallowed, not propagated.

Usage:
    ledger = Ledger()                      # in-memory, hermetic
    ledger = Ledger("checkpoints/runs.db") # persisted to disk
    ceo = CEO(registry, ledger=ledger)
"""

from __future__ import annotations

import json
import sqlite3
import threading
import time
import uuid
from pathlib import Path
from typing import Any, Dict, List, Optional

_SCHEMA = """
CREATE TABLE IF NOT EXISTS events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    run_id TEXT NOT NULL,
    seq INTEGER NOT NULL,
    ts REAL NOT NULL,
    kind TEXT NOT NULL,
    actor TEXT NOT NULL,
    payload TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_events_run ON events (run_id, seq);
"""


def new_run_id() -> str:
    """Mint an id for one CEO.handle() or CEO.team() call."""
    return uuid.uuid4().hex[:12]


class Ledger:
    """Append-only, SQLite-backed event log for agent runs."""

    def __init__(self, path: Optional[str | Path] = None) -> None:
        # None means in-memory: hermetic, no disk side effects. Pass a
        # path to persist the trail across processes.
        self.path = str(path) if path else ":memory:"
        if self.path != ":memory:":
            Path(self.path).parent.mkdir(parents=True, exist_ok=True)
        self._conn = sqlite3.connect(self.path)
        self._conn.executescript(_SCHEMA)
        self._lock = threading.Lock()
        self._seq: Dict[str, int] = {}

    def record(
        self,
        run_id: str,
        kind: str,
        actor: str,
        payload: Optional[Dict[str, Any]] = None,
    ) -> int:
        """Append one event. Returns the event row id."""
        with self._lock:
            seq = self._seq.get(run_id, 0) + 1
            self._seq[run_id] = seq
            cur = self._conn.execute(
                "INSERT INTO events (run_id, seq, ts, kind, actor, payload)"
                " VALUES (?, ?, ?, ?, ?, ?)",
                (
                    run_id,
                    seq,
                    time.time(),
                    kind,
                    actor,
                    json.dumps(payload or {}, default=str),
                ),
            )
            self._conn.commit()
            return cur.lastrowid

    def events(self, run_id: str) -> List[Dict[str, Any]]:
        """All events for one run, in order. This is the replay."""
        cur = self._conn.execute(
            "SELECT seq, ts, kind, actor, payload FROM events"
            " WHERE run_id = ? ORDER BY seq",
            (run_id,),
        )
        return [
            {
                "seq": seq,
                "ts": ts,
                "kind": kind,
                "actor": actor,
                "payload": json.loads(payload),
            }
            for seq, ts, kind, actor, payload in cur.fetchall()
        ]

    def runs(self) -> List[Dict[str, Any]]:
        """One row per run: id, event count, first and last timestamp."""
        cur = self._conn.execute(
            "SELECT run_id, COUNT(*), MIN(ts), MAX(ts) FROM events"
            " GROUP BY run_id ORDER BY MAX(ts) DESC"
        )
        return [
            {"run_id": run_id, "events": count, "started": first, "ended": last}
            for run_id, count, first, last in cur.fetchall()
        ]

    def close(self) -> None:
        """Close the underlying connection. Record after this raises."""
        self._conn.close()

    def __enter__(self) -> "Ledger":
        return self

    def __exit__(self, *exc: Any) -> None:
        self.close()
