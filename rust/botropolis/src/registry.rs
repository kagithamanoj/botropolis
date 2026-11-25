//! AgentRegistry: the company directory of Botropolis.
//!
//! Discovers every agent spec under `botropolis/agents/<department>/*.yaml`,
//! instantiates an Agent per spec, and answers lookups by name or
//! department. Mirrors `botropolis/core/registry.py`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use thiserror::Error;

use crate::agent::{Agent, AgentError};
use crate::models::{ModelClient, RealClient};
use crate::paths::default_agents_dir;

#[derive(Debug, Error)]
pub enum RegistryError {
    #[error(transparent)]
    Agent(#[from] AgentError),
    #[error("Unknown agent: {0}")]
    UnknownAgent(String),
    #[error("Duplicate agent name: {0}")]
    Duplicate(String),
}

/// Discovers and serves all registered agents.
pub struct AgentRegistry {
    pub agents_dir: PathBuf,
    agents: BTreeMap<String, Arc<Agent>>,
}

impl std::fmt::Debug for AgentRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentRegistry")
            .field("agents_dir", &self.agents_dir)
            .field("agents", &self.agents.len())
            .finish()
    }
}

impl AgentRegistry {
    /// Load every spec under the default agents directory.
    pub fn load() -> Result<AgentRegistry, RegistryError> {
        AgentRegistry::load_from(default_agents_dir(), None)
    }

    /// Load every spec under `agents_dir`, sharing one client.
    pub fn load_from(
        agents_dir: impl AsRef<Path>,
        client: Option<Arc<dyn ModelClient>>,
    ) -> Result<AgentRegistry, RegistryError> {
        let agents_dir = agents_dir.as_ref().to_path_buf();
        let client = client.unwrap_or_else(|| Arc::new(RealClient::default()));
        let mut agents: BTreeMap<String, Arc<Agent>> = BTreeMap::new();
        let mut spec_paths: Vec<PathBuf> = Vec::new();
        if let Ok(entries) = std::fs::read_dir(&agents_dir) {
            for entry in entries.flatten() {
                let dept_dir = entry.path();
                if !dept_dir.is_dir() {
                    continue;
                }
                if let Ok(files) = std::fs::read_dir(&dept_dir) {
                    for file in files.flatten() {
                        let path = file.path();
                        if path.extension().and_then(|e| e.to_str()) == Some("yaml") {
                            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                                if !name.starts_with('_') {
                                    spec_paths.push(path);
                                }
                            }
                        }
                    }
                }
            }
        }
        spec_paths.sort();
        for spec_path in spec_paths {
            let agent = Agent::load(&spec_path, Some(Arc::clone(&client)))?;
            let key = agent.name.to_lowercase();
            if agents.contains_key(&key) {
                return Err(RegistryError::Duplicate(agent.name.clone()));
            }
            agents.insert(key, Arc::new(agent));
        }
        Ok(AgentRegistry { agents_dir, agents })
    }

    /// Return the agent with this name (case-insensitive).
    pub fn get(&self, name: &str) -> Result<Arc<Agent>, RegistryError> {
        self.agents
            .get(&name.to_lowercase())
            .cloned()
            .ok_or_else(|| RegistryError::UnknownAgent(name.to_string()))
    }

    /// All agents in a department, sorted by name.
    pub fn by_department(&self, department: &str) -> Vec<Arc<Agent>> {
        let dept = department.to_lowercase();
        let mut found: Vec<Arc<Agent>> = self
            .agents
            .values()
            .filter(|a| a.department.to_lowercase() == dept)
            .cloned()
            .collect();
        found.sort_by(|a, b| a.name.cmp(&b.name));
        found
    }

    /// Every agent, sorted by department then name.
    pub fn list_all(&self) -> Vec<Arc<Agent>> {
        let mut all: Vec<Arc<Agent>> = self.agents.values().cloned().collect();
        all.sort_by(|a, b| {
            a.department
                .to_lowercase()
                .cmp(&b.department.to_lowercase())
                .then(a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        all
    }

    /// The sorted list of department names.
    pub fn departments(&self) -> Vec<String> {
        let mut depts: Vec<String> = self
            .agents
            .values()
            .map(|a| a.department.clone())
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect();
        depts.sort();
        depts
    }

    pub fn len(&self) -> usize {
        self.agents.len()
    }

    pub fn is_empty(&self) -> bool {
        self.agents.is_empty()
    }

    pub fn contains(&self, name: &str) -> bool {
        self.agents.contains_key(&name.to_lowercase())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_registry_has_twenty_agents() {
        let registry = AgentRegistry::load().unwrap();
        assert_eq!(registry.len(), 20);
        assert_eq!(registry.departments().len(), 10);
        assert_eq!(registry.by_department("research").len(), 3);
        assert_eq!(registry.by_department("code").len(), 3);
        assert_eq!(registry.by_department("legal").len(), 1);
    }

    #[test]
    fn lookup_is_case_insensitive() {
        let registry = AgentRegistry::load().unwrap();
        assert_eq!(registry.get("CODER").unwrap().name, "coder");
        assert!(registry.get("nobody").is_err());
    }
}
