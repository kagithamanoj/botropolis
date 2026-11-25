//! Botropolis in Rust: the same company of bots as the Python side.
//!
//! The Rust code loads the same agent YAML specs from
//! `botropolis/agents/`, serves the same web UI from `botropolis/web/`,
//! and writes run state, task maps, and ledger rows in the same formats,
//! so a run started by either side can be read by the other.

pub mod agent;
pub mod models;
pub mod output_schemas;
pub mod paths;
pub mod registry;
pub mod schemas;
pub mod tools;
