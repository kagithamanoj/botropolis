//! Append-only run ledger for Botropolis.
//!
//! Every agent run appends events to a single ledger, SQLite-backed
//! with the same table as the Python side (`botropolis/core/ledger.py`).
//! The ledger is the durable trail of what happened: tasks assigned,
//! tools called, answers produced, contracts repaired, runs finished.
//! The CompanyReport remains the API's answer; the ledger is the
//! observable, replayable record underneath it, and the event stream
//! doubles as training and eval data.
//!
//! The ledger is a side channel: recording never breaks a run. A
//! failing ledger is swallowed, not propagated.

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

const SCHEMA: &str = "
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
";

fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

/// One ledger event, in replay order.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LedgerEvent {
    pub seq: i64,
    pub ts: f64,
    pub kind: String,
    pub actor: String,
    pub payload: Value,
}

/// One row per run: id, event count, first and last timestamp.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunSummary {
    pub run_id: String,
    pub events: i64,
    pub started: f64,
    pub ended: f64,
}

/// Append-only, SQLite-backed event log for agent runs.
#[derive(Clone)]
pub struct Ledger {
    conn: Arc<Mutex<Connection>>,
    path: String,
    seq: Arc<Mutex<HashMap<String, i64>>>,
}

impl std::fmt::Debug for Ledger {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Ledger").field("path", &self.path).finish()
    }
}

impl Ledger {
    /// Open a ledger at a path, creating it when needed.
    pub fn open(path: impl AsRef<Path>) -> Result<Ledger, String> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
        }
        let conn = Connection::open(path).map_err(|e| e.to_string())?;
        conn.execute_batch(SCHEMA).map_err(|e| e.to_string())?;
        Ok(Ledger {
            conn: Arc::new(Mutex::new(conn)),
            path: path.to_string_lossy().to_string(),
            seq: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    /// An in-memory ledger: hermetic, no disk side effects.
    pub fn in_memory() -> Ledger {
        let conn = Connection::open_in_memory().expect("in-memory sqlite");
        conn.execute_batch(SCHEMA).expect("ledger schema");
        Ledger {
            conn: Arc::new(Mutex::new(conn)),
            path: ":memory:".to_string(),
            seq: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Append one event. Returns the event row id.
    pub fn record(
        &self,
        run_id: &str,
        kind: &str,
        actor: &str,
        payload: Value,
    ) -> Result<i64, String> {
        let seq = {
            let mut seqs = self.seq.lock().map_err(|e| e.to_string())?;
            let next = seqs.get(run_id).copied().unwrap_or(0) + 1;
            seqs.insert(run_id.to_string(), next);
            next
        };
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT INTO events (run_id, seq, ts, kind, actor, payload) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                run_id,
                seq,
                now_secs(),
                kind,
                actor,
                serde_json::to_string(&payload).unwrap_or_else(|_| "{}".to_string()),
            ],
        )
        .map_err(|e| e.to_string())?;
        Ok(conn.last_insert_rowid())
    }

    /// All events for one run, in order. This is the replay.
    pub fn events(&self, run_id: &str) -> Result<Vec<LedgerEvent>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let mut stmt = conn
            .prepare(
                "SELECT seq, ts, kind, actor, payload FROM events WHERE run_id = ?1 ORDER BY seq",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(params![run_id], |row| {
                let payload_text: String = row.get(4)?;
                Ok(LedgerEvent {
                    seq: row.get(0)?,
                    ts: row.get(1)?,
                    kind: row.get(2)?,
                    actor: row.get(3)?,
                    payload: serde_json::from_str(&payload_text).unwrap_or(Value::Null),
                })
            })
            .map_err(|e| e.to_string())?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(|e| e.to_string())?);
        }
        Ok(out)
    }

    /// One row per run: id, event count, first and last timestamp.
    pub fn runs(&self) -> Result<Vec<RunSummary>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let mut stmt = conn
            .prepare(
                "SELECT run_id, COUNT(*), MIN(ts), MAX(ts) FROM events GROUP BY run_id ORDER BY MAX(ts) DESC",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |row| {
                Ok(RunSummary {
                    run_id: row.get(0)?,
                    events: row.get(1)?,
                    started: row.get(2)?,
                    ended: row.get(3)?,
                })
            })
            .map_err(|e| e.to_string())?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(|e| e.to_string())?);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn record_and_replay_in_order() {
        let ledger = Ledger::in_memory();
        ledger
            .record("run1", "task_assigned", "coder", json!({"task": "x"}))
            .unwrap();
        ledger
            .record(
                "run1",
                "tool_called",
                "coder",
                json!({"tool": "calculator"}),
            )
            .unwrap();
        ledger
            .record("run2", "task_assigned", "scout", json!({}))
            .unwrap();
        let events = ledger.events("run1").unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].kind, "task_assigned");
        assert_eq!(events[1].kind, "tool_called");
        assert_eq!(events[0].seq, 1);
        assert_eq!(events[1].seq, 2);
        let runs = ledger.runs().unwrap();
        assert_eq!(runs.len(), 2);
        assert_eq!(runs.iter().find(|r| r.run_id == "run1").unwrap().events, 2);
    }
}
