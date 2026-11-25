//! Long-term memory for Botropolis.
//!
//! The notebook is a shared page of notes. Memory is the layer under
//! it: individual facts, saved by whichever agent learned them, that
//! later runs can search. A memory is one durable fact: a decision, a
//! name, a number, a preference, something a future task will need and
//! should not have to rediscover.
//!
//! Storage is SQLite via rusqlite, with the same table as the Python
//! side (`botropolis/core/memory.py`), so either side can read the
//! other's memories. The default store lives at
//! `botropolis/data/memory.db` (gitignored); `BOTROPOLIS_MEMORY_DB`
//! moves it, which is how tests and evals stay hermetic.
//! `MemoryStore::in_memory()` is hermetic with no disk side effects.
//!
//! Recall is deliberately simple: keyword overlap between the query
//! and the fact (text plus tags), weighted by the importance the saver
//! gave it, newest first when scores tie. Recalling a memory counts as
//! using it: access_count goes up and last_accessed_at moves.

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::env;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

const SCHEMA: &str = "
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
";

fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

/// One durable fact in the company memory.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Memory {
    pub id: String,
    pub text: String,
    pub agent: String,
    #[serde(default = "default_importance")]
    pub importance: i64,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub created_at: f64,
    #[serde(default)]
    pub access_count: i64,
    #[serde(default)]
    pub last_accessed_at: Option<f64>,
}

fn default_importance() -> i64 {
    3
}

fn terms(text: &str) -> std::collections::HashSet<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(|t| t.to_string())
        .collect()
}

/// SQLite-backed store of Memory rows.
#[derive(Clone)]
pub struct MemoryStore {
    conn: Arc<Mutex<Connection>>,
    path: String,
}

impl std::fmt::Debug for MemoryStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MemoryStore")
            .field("path", &self.path)
            .finish()
    }
}

impl MemoryStore {
    /// Open a store at a path, creating it when needed.
    pub fn open(path: impl AsRef<Path>) -> Result<MemoryStore, String> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
        }
        let conn = Connection::open(path).map_err(|e| e.to_string())?;
        conn.execute_batch(SCHEMA).map_err(|e| e.to_string())?;
        Ok(MemoryStore {
            conn: Arc::new(Mutex::new(conn)),
            path: path.to_string_lossy().to_string(),
        })
    }

    /// An in-memory store: hermetic, no disk side effects.
    pub fn in_memory() -> MemoryStore {
        let conn = Connection::open_in_memory().expect("in-memory sqlite");
        conn.execute_batch(SCHEMA).expect("memory schema");
        MemoryStore {
            conn: Arc::new(Mutex::new(conn)),
            path: ":memory:".to_string(),
        }
    }

    /// Store one fact. Returns the saved Memory.
    pub fn save(
        &self,
        text: &str,
        agent: &str,
        importance: i64,
        tags: &[String],
    ) -> Result<Memory, String> {
        let importance = importance.clamp(1, 5);
        let memory = Memory {
            id: crate::schemas::new_run_id(),
            text: text.trim().to_string(),
            agent: agent.to_string(),
            importance,
            tags: tags
                .iter()
                .map(|t| t.trim().to_string())
                .filter(|t| !t.is_empty())
                .collect(),
            created_at: now_secs(),
            access_count: 0,
            last_accessed_at: None,
        };
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT INTO memories (id, text, agent, importance, tags, created_at, access_count, last_accessed_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0, NULL)",
            params![
                memory.id,
                memory.text,
                memory.agent,
                memory.importance,
                memory.tags.join("\n"),
                memory.created_at,
            ],
        )
        .map_err(|e| e.to_string())?;
        Ok(memory)
    }

    /// Best matching memories for a query, most relevant first.
    ///
    /// Score is keyword overlap (query terms found in the fact's text
    /// or tags) weighted by importance; ties go to the newest memory.
    /// Memories with no overlap are not returned. Returned memories
    /// have their access stats updated before they come back.
    pub fn recall(&self, query: &str, limit: usize) -> Result<Vec<Memory>, String> {
        let query_terms = terms(query);
        if query_terms.is_empty() {
            return Ok(Vec::new());
        }
        let limit = limit.max(1);
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let mut stmt = conn
            .prepare(
                "SELECT id, text, agent, importance, tags, created_at, access_count, last_accessed_at FROM memories",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, f64>(5)?,
                    row.get::<_, i64>(6)?,
                    row.get::<_, Option<f64>>(7)?,
                ))
            })
            .map_err(|e| e.to_string())?;
        let mut scored: Vec<(i64, Memory)> = Vec::new();
        for row in rows {
            let (id, text, agent, importance, tags, created_at, access_count, last) =
                row.map_err(|e| e.to_string())?;
            let memory = Memory {
                id,
                text,
                agent,
                importance,
                tags: if tags.is_empty() {
                    Vec::new()
                } else {
                    tags.split('\n').map(|t| t.to_string()).collect()
                },
                created_at,
                access_count,
                last_accessed_at: last,
            };
            let mut haystack = terms(&memory.text);
            haystack.extend(terms(&memory.tags.join(" ")));
            let overlap = query_terms.intersection(&haystack).count() as i64;
            if overlap > 0 {
                scored.push((overlap * memory.importance, memory));
            }
        }
        // Highest score first; ties go to the newest memory.
        scored.sort_by(|a, b| {
            b.0.cmp(&a.0).then(
                b.1.created_at
                    .partial_cmp(&a.1.created_at)
                    .unwrap_or(std::cmp::Ordering::Equal),
            )
        });
        let mut top: Vec<Memory> = scored.into_iter().take(limit).map(|(_, m)| m).collect();
        let now = now_secs();
        for memory in &mut top {
            conn.execute(
                "UPDATE memories SET access_count = access_count + 1, last_accessed_at = ?1 WHERE id = ?2",
                params![now, memory.id],
            )
            .map_err(|e| e.to_string())?;
            memory.access_count += 1;
            memory.last_accessed_at = Some(now);
        }
        Ok(top)
    }

    /// Every memory, newest first. For inspection and tests.
    pub fn all(&self) -> Result<Vec<Memory>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let mut stmt = conn
            .prepare(
                "SELECT id, text, agent, importance, tags, created_at, access_count, last_accessed_at FROM memories ORDER BY created_at DESC",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |row| {
                Ok(Memory {
                    id: row.get(0)?,
                    text: row.get(1)?,
                    agent: row.get(2)?,
                    importance: row.get(3)?,
                    tags: {
                        let tags: String = row.get(4)?;
                        if tags.is_empty() {
                            Vec::new()
                        } else {
                            tags.split('\n').map(|t| t.to_string()).collect()
                        }
                    },
                    created_at: row.get(5)?,
                    access_count: row.get(6)?,
                    last_accessed_at: row.get(7)?,
                })
            })
            .map_err(|e| e.to_string())?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(|e| e.to_string())?);
        }
        Ok(out)
    }

    /// How many memories the store holds.
    pub fn count(&self) -> Result<i64, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM memories", [], |row| row.get(0))
            .map_err(|e| e.to_string())?;
        Ok(count)
    }
}

/// Where the shared company memory lives on disk.
///
/// `BOTROPOLIS_MEMORY_DB` overrides it (a file path, or ":memory:").
pub fn default_memory_path() -> PathBuf {
    if let Ok(override_path) = env::var("BOTROPOLIS_MEMORY_DB") {
        if !override_path.is_empty() {
            return PathBuf::from(override_path);
        }
    }
    crate::paths::data_dir().join("memory.db")
}

/// A fresh store at the default path.
pub fn default_memory_store() -> MemoryStore {
    let path = default_memory_path();
    if path.to_string_lossy() == ":memory:" {
        MemoryStore::in_memory()
    } else {
        MemoryStore::open(path).unwrap_or_else(|_| MemoryStore::in_memory())
    }
}

static PINNED_STORE: OnceLock<Mutex<Option<MemoryStore>>> = OnceLock::new();
static DEFAULT_STORE: OnceLock<Mutex<(Option<MemoryStore>, Option<String>)>> = OnceLock::new();

fn pinned_slot() -> &'static Mutex<Option<MemoryStore>> {
    PINNED_STORE.get_or_init(|| Mutex::new(None))
}

fn default_slot() -> &'static Mutex<(Option<MemoryStore>, Option<String>)> {
    DEFAULT_STORE.get_or_init(|| Mutex::new((None, None)))
}

/// Pin the shared store (server, CLI, tests). None unpins it.
pub fn set_memory_store(store: Option<MemoryStore>) {
    if let Ok(mut slot) = pinned_slot().lock() {
        *slot = store;
    }
}

/// The shared store, resolved at call time like the other
/// `BOTROPOLIS_*` overrides: an explicit `BOTROPOLIS_MEMORY_DB` wins,
/// then a pinned store, then the store at the default path (rebuilt
/// when the path changes).
pub fn get_memory_store() -> MemoryStore {
    if let Ok(override_path) = env::var("BOTROPOLIS_MEMORY_DB") {
        if !override_path.is_empty() {
            let path = override_path.clone();
            if let Ok(mut slot) = default_slot().lock() {
                if slot.0.is_none() || slot.1.as_deref() != Some(path.as_str()) {
                    slot.0 = Some(if path == ":memory:" {
                        MemoryStore::in_memory()
                    } else {
                        MemoryStore::open(&path).unwrap_or_else(|_| MemoryStore::in_memory())
                    });
                    slot.1 = Some(path);
                }
                if let Some(store) = &slot.0 {
                    return store.clone();
                }
            }
        }
    }
    if let Ok(slot) = pinned_slot().lock() {
        if let Some(store) = &*slot {
            return store.clone();
        }
    }
    let path = default_memory_path().to_string_lossy().to_string();
    if let Ok(mut slot) = default_slot().lock() {
        if slot.0.is_none() || slot.1.as_deref() != Some(path.as_str()) {
            slot.0 = Some(default_memory_store());
            slot.1 = Some(path);
        }
        if let Some(store) = &slot.0 {
            return store.clone();
        }
    }
    MemoryStore::in_memory()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_and_recall_scores_overlap_times_importance() {
        let store = MemoryStore::in_memory();
        store
            .save("Deploys freeze on Fridays", "scheduler", 4, &[])
            .unwrap();
        store
            .save("The office dog is named Biscuit", "support", 3, &[])
            .unwrap();
        store
            .save("Friday deploys need a freeze window", "coder", 5, &[])
            .unwrap();
        let found = store.recall("deploy freeze friday", 5).unwrap();
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].text, "Friday deploys need a freeze window");
        assert_eq!(found[0].access_count, 1);
        assert!(found[0].last_accessed_at.is_some());
        assert!(store.recall("quantum banana", 5).unwrap().is_empty());
    }

    #[test]
    fn importance_is_clamped() {
        let store = MemoryStore::in_memory();
        let saved = store.save("a fact", "coder", 99, &[]).unwrap();
        assert_eq!(saved.importance, 5);
        assert_eq!(store.count().unwrap(), 1);
    }
}
