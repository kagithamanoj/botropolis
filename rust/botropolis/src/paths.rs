//! Finding the repository root, so the Rust side can load the same
//! agent specs, model registry, and web UI the Python side uses.
//!
//! `BOTROPOLIS_REPO_ROOT` wins when set. Otherwise walk up from the
//! current directory looking for a directory that holds
//! `botropolis/agents`. When nothing matches, the current directory
//! is the root.

use std::env;
use std::path::{Path, PathBuf};

/// Return the repository root.
pub fn repo_root() -> PathBuf {
    if let Ok(root) = env::var("BOTROPOLIS_REPO_ROOT") {
        if !root.is_empty() {
            return PathBuf::from(root);
        }
    }
    let mut dir = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    loop {
        if dir.join("botropolis").join("agents").is_dir() {
            return dir;
        }
        match dir.parent() {
            Some(parent) => dir = parent.to_path_buf(),
            None => return env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
        }
    }
}

/// The bundled agent specs directory.
pub fn default_agents_dir() -> PathBuf {
    repo_root().join("botropolis").join("agents")
}

/// The model registry file, `models/registry.yaml`.
pub fn model_registry_path() -> PathBuf {
    repo_root().join("models").join("registry.yaml")
}

/// The directory holding the web UI files.
pub fn web_dir() -> PathBuf {
    repo_root().join("botropolis").join("web")
}

/// The data directory (`botropolis/data/`).
///
/// `BOTROPOLIS_DATA_DIR` moves it, which is how hosted tests keep
/// tenant state hermetic.
pub fn data_dir() -> PathBuf {
    if let Ok(dir) = env::var("BOTROPOLIS_DATA_DIR") {
        if !dir.is_empty() {
            return PathBuf::from(dir);
        }
    }
    repo_root().join("botropolis").join("data")
}

/// The platform database (`botropolis/data/platform.db`), holding
/// teams, API keys, and sessions. `BOTROPOLIS_PLATFORM_DB` moves it.
pub fn platform_db_path() -> PathBuf {
    if let Ok(path) = env::var("BOTROPOLIS_PLATFORM_DB") {
        if !path.is_empty() {
            return PathBuf::from(path);
        }
    }
    data_dir().join("platform.db")
}

/// The directory holding one tenant's isolated state.
pub fn tenant_dir(tenant_id: &str) -> PathBuf {
    data_dir()
        .join("tenants")
        .join(crate::tenancy::sanitize_tenant_id(tenant_id))
}

/// One tenant's memory database.
pub fn tenant_memory_path(tenant_id: &str) -> PathBuf {
    tenant_dir(tenant_id).join("memory.db")
}

/// One tenant's run ledger database.
pub fn tenant_ledger_path(tenant_id: &str) -> PathBuf {
    tenant_dir(tenant_id).join("ledger.db")
}

/// One tenant's run state directory.
pub fn tenant_runs_dir(tenant_id: &str) -> PathBuf {
    tenant_dir(tenant_id).join("runs")
}

/// One tenant's notebook file.
pub fn tenant_notebook_path(tenant_id: &str) -> PathBuf {
    tenant_dir(tenant_id).join("notes.md")
}

/// One tenant's usage log.
pub fn tenant_usage_log_path(tenant_id: &str) -> PathBuf {
    tenant_dir(tenant_id).join("usage.jsonl")
}

pub fn is_repo_root(path: &Path) -> bool {
    path.join("botropolis").join("agents").is_dir()
}
