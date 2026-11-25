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
pub fn data_dir() -> PathBuf {
    repo_root().join("botropolis").join("data")
}

pub fn is_repo_root(path: &Path) -> bool {
    path.join("botropolis").join("agents").is_dir()
}
