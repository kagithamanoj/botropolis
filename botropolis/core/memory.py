"""Long-term memory for Botropolis.

The notebook is a shared page of notes. Memory is the layer under it:
individual facts, saved by whichever agent learned them, that later
runs can search. A memory is one durable fact: a decision, a name, a
number, a preference, something a future task will need and should not
have to rediscover.

Storage is SQLite via the standard library, the same pattern as the
run ledger. The default store lives at botropolis/data/memory.db
(gitignored); BOTROPOLIS_MEMORY_DB moves it, which is how tests and
evals stay hermetic. MemoryStore() with no path is in-memory.

Recall is deliberately simple: keyword overlap between the query and
the fact (text plus tags), weighted by the importance the saver gave
it, newest first when scores tie. Recalling a memory counts as using
it: access_count goes up and last_accessed_at moves, so the store can
show which facts the company actually leans on.

Usage:
    store = MemoryStore()                    # in-memory, hermetic
    store = MemoryStore("botropolis/data/memory.db")
    store.save("Deploys freeze on Fridays", agent="scheduler", importance=4)
    store.recall("deploy freeze")
"""

from __future__ import annotations

import os
import re
import sqlite3
import threading
import time
import uuid
from dataclasses import asdict, dataclass, field
from pathlib import Path
from typing import Any, Dict, List, Optional, Union

_SCHEMA = """
CREATE TABLE IF NOT EXISTS memories (
    id TEXT PRIMARY KEY,
    text TEXT NOT NULL,
    agent TEXT NOT NULL,
    importance INTEGER NOT NULL,
    tags TEXT NOT NULL,
    created_at REAL NOT NULL,
    access_count INTEGER NOT NULL DEFAULT 0,
    last_accessed_at REAL
);
CREATE INDEX IF NOT EXISTS idx_memories_created ON memories (created_at);
"""

_TOKEN = re.compile(r"[a-z0-9]+")


def _terms(text: str) -> set:
    """Lowercase word tokens of a string, for keyword overlap."""
    return set(_TOKEN.findall((text or "").lower()))


@dataclass
class Memory:
    """One durable fact in the company memory."""

    id: str
    text: str
    agent: str
    importance: int = 3
    tags: List[str] = field(default_factory=list)
    created_at: float = 0.0
    access_count: int = 0
    last_accessed_at: Optional[float] = None

    def to_dict(self) -> Dict[str, Any]:
        """Return a JSON-serializable dict."""
        return asdict(self)


class MemoryStore:
    """SQLite-backed store of Memory rows."""

    def __init__(self, path: Optional[Union[str, Path]] = None) -> None:
        # None means in-memory: hermetic, no disk side effects. Pass a
        # path to keep memories across processes.
        self.path = str(path) if path else ":memory:"
        if self.path != ":memory:":
            Path(self.path).parent.mkdir(parents=True, exist_ok=True)
        self._conn = sqlite3.connect(self.path, check_same_thread=False)
        self._conn.executescript(_SCHEMA)
        self._conn.commit()
        self._lock = threading.Lock()

    def save(
        self,
        text: str,
        agent: str = "",
        importance: int = 3,
        tags: Optional[List[str]] = None,
    ) -> Memory:
        """Store one fact. Returns the saved Memory."""
        importance = max(1, min(5, int(importance)))
        memory = Memory(
            id=uuid.uuid4().hex[:12],
            text=(text or "").strip(),
            agent=agent or "",
            importance=importance,
            tags=[str(tag).strip() for tag in (tags or []) if str(tag).strip()],
            created_at=time.time(),
        )
        with self._lock:
            self._conn.execute(
                "INSERT INTO memories"
                " (id, text, agent, importance, tags, created_at,"
                "  access_count, last_accessed_at)"
                " VALUES (?, ?, ?, ?, ?, ?, 0, NULL)",
                (
                    memory.id,
                    memory.text,
                    memory.agent,
                    memory.importance,
                    "\n".join(memory.tags),
                    memory.created_at,
                ),
            )
            self._conn.commit()
        return memory

    def recall(self, query: str, limit: int = 5) -> List[Memory]:
        """Best matching memories for a query, most relevant first.

        Score is keyword overlap (query terms found in the fact's text
        or tags) weighted by importance; ties go to the newest memory.
        Memories with no overlap are not returned. Returned memories
        have their access stats updated before they come back.
        """
        query_terms = _terms(query)
        if not query_terms:
            return []
        limit = max(1, int(limit))
        with self._lock:
            rows = self._conn.execute(
                "SELECT id, text, agent, importance, tags, created_at,"
                " access_count, last_accessed_at FROM memories"
            ).fetchall()
            scored = []
            for row in rows:
                memory = self._row_to_memory(row)
                haystack = _terms(memory.text) | _terms(" ".join(memory.tags))
                overlap = len(query_terms & haystack)
                if overlap:
                    scored.append((overlap * memory.importance, memory))
            scored.sort(key=lambda item: (-item[0], -item[1].created_at))
            top = [memory for _, memory in scored[:limit]]
            now = time.time()
            for memory in top:
                self._conn.execute(
                    "UPDATE memories SET access_count = access_count + 1,"
                    " last_accessed_at = ? WHERE id = ?",
                    (now, memory.id),
                )
                memory.access_count += 1
                memory.last_accessed_at = now
            self._conn.commit()
        return top

    def all(self) -> List[Memory]:
        """Every memory, newest first. For inspection and tests."""
        with self._lock:
            rows = self._conn.execute(
                "SELECT id, text, agent, importance, tags, created_at,"
                " access_count, last_accessed_at FROM memories"
                " ORDER BY created_at DESC"
            ).fetchall()
        return [self._row_to_memory(row) for row in rows]

    def count(self) -> int:
        """How many memories the store holds."""
        with self._lock:
            row = self._conn.execute("SELECT COUNT(*) FROM memories").fetchone()
        return int(row[0])

    def close(self) -> None:
        """Close the underlying connection."""
        self._conn.close()

    def __enter__(self) -> "MemoryStore":
        return self

    def __exit__(self, *exc: Any) -> None:
        self.close()

    @staticmethod
    def _row_to_memory(row) -> Memory:
        mid, text, agent, importance, tags, created_at, access_count, last = row
        return Memory(
            id=mid,
            text=text,
            agent=agent,
            importance=int(importance),
            tags=tags.split("\n") if tags else [],
            created_at=float(created_at),
            access_count=int(access_count),
            last_accessed_at=float(last) if last is not None else None,
        )


def default_memory_path() -> Path:
    """Where the shared company memory lives on disk.

    BOTROPOLIS_MEMORY_DB overrides it (a file path, or ":memory:").
    """
    override = os.environ.get("BOTROPOLIS_MEMORY_DB")
    if override:
        return Path(override).expanduser()
    return Path(__file__).resolve().parents[1] / "data" / "memory.db"


def default_memory_store() -> MemoryStore:
    """A fresh store at the default path. Callers that need the one
    shared instance should use get_memory_store() instead."""
    path = default_memory_path()
    if str(path) == ":memory:":
        return MemoryStore()
    return MemoryStore(path)


_pinned_store: Optional[MemoryStore] = None
_default_store: Optional[MemoryStore] = None
_default_path: Optional[str] = None


def set_memory_store(store: Optional[MemoryStore]) -> None:
    """Pin the shared store (server, CLI, tests). None unpins it."""
    global _pinned_store
    _pinned_store = store


def get_memory_store() -> MemoryStore:
    """The shared store, resolved at call time like the other
    BOTROPOLIS_* overrides: an explicit BOTROPOLIS_MEMORY_DB wins,
    then a pinned store, then the store at the default path (rebuilt
    when the path changes)."""
    global _default_store, _default_path
    override = os.environ.get("BOTROPOLIS_MEMORY_DB")
    if override:
        path = str(Path(override).expanduser())
        if _default_store is None or _default_path != path:
            _default_store = (
                MemoryStore() if path == ":memory:" else MemoryStore(path)
            )
            _default_path = path
        return _default_store
    if _pinned_store is not None:
        return _pinned_store
    path = str(default_memory_path())
    if _default_store is None or _default_path != path:
        _default_store = default_memory_store()
        _default_path = path
    return _default_store
