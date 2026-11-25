//! Botropolis in Rust: the same company of bots as the Python side.
//!
//! The Rust code loads the same agent YAML specs from
//! `botropolis/agents/`, serves the same web UI from `botropolis/web/`,
//! and writes run state, task maps, and ledger rows in the same formats,
//! so a run started by either side can be read by the other.

pub mod agent;
pub mod evidence;
pub mod judge;
pub mod ledger;
pub mod memory;
pub mod models;
pub mod output_schemas;
pub mod paths;
pub mod registry;
pub mod runstate;
pub mod sandbox;
pub mod schemas;
pub mod taskmap;
pub mod tool_loop;
pub mod tools;
pub mod trace;

/// Helpers shared by tests that touch process-global environment
/// variables. Tests run in parallel, so env mutation needs a lock.
#[cfg(test)]
pub mod test_support {
    use std::sync::{Mutex, MutexGuard};

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    pub fn lock_env() -> MutexGuard<'static, ()> {
        ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }
}
