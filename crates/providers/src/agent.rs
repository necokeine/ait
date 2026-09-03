use std::{collections::HashMap, sync::RwLock};

use serde::{Deserialize, Serialize};

use crate::{CredentialRef, ProviderCapabilities, ProviderParameters};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentDefinition {
    pub id: String,
    pub name: String,
    pub driver: String,
    pub model: String,
    pub endpoint: Option<String>,
    pub credential_ref: Option<CredentialRef>,
    pub capabilities: ProviderCapabilities,
    #[serde(default)]
    pub default_parameters: ProviderParameters,
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentRevision {
    pub agent_id: String,
    pub revision: u64,
    pub definition: AgentDefinition,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum CatalogError {
    #[error("agent id cannot be empty")]
    EmptyId,
    #[error("agent not found: {0}")]
    NotFound(String),
    #[error("agent revision not found: {agent_id}@{revision}")]
    RevisionNotFound { agent_id: String, revision: u64 },
    #[error("agent is disabled: {0}")]
    Disabled(String),
}

/// Append-only version catalog. Publishing never mutates an old revision.
#[derive(Debug, Default)]
pub struct AgentCatalog {
    revisions: RwLock<HashMap<String, Vec<AgentRevision>>>,
}

impl AgentCatalog {
    /// Appends a new immutable revision for an agent definition.
    ///
    /// # Errors
    ///
    /// Returns [`CatalogError::EmptyId`] when the definition has no identifier.
    ///
    /// # Panics
    ///
    /// Panics if an earlier thread poisoned the in-memory catalog lock.
    pub fn publish(&self, definition: AgentDefinition) -> Result<AgentRevision, CatalogError> {
        if definition.id.trim().is_empty() {
            return Err(CatalogError::EmptyId);
        }
        let mut all = self.revisions.write().expect("agent catalog lock poisoned");
        let versions = all.entry(definition.id.clone()).or_default();
        let revision = versions.last().map_or(1, |item| item.revision + 1);
        let item = AgentRevision {
            agent_id: definition.id.clone(),
            revision,
            definition,
        };
        versions.push(item.clone());
        Ok(item)
    }

    /// Resolve exactly once when a Run is created; persist both returned keys on the Run.
    ///
    /// # Errors
    ///
    /// Returns an error when the agent or revision does not exist, or when the
    /// selected definition is disabled.
    ///
    /// # Panics
    ///
    /// Panics if an earlier thread poisoned the in-memory catalog lock.
    pub fn pin(
        &self,
        agent_id: &str,
        revision: Option<u64>,
    ) -> Result<AgentRevision, CatalogError> {
        let all = self.revisions.read().expect("agent catalog lock poisoned");
        let versions = all
            .get(agent_id)
            .ok_or_else(|| CatalogError::NotFound(agent_id.to_owned()))?;
        let item = match revision {
            Some(revision) => versions
                .iter()
                .find(|item| item.revision == revision)
                .ok_or_else(|| CatalogError::RevisionNotFound {
                    agent_id: agent_id.to_owned(),
                    revision,
                })?,
            None => versions
                .last()
                .ok_or_else(|| CatalogError::NotFound(agent_id.to_owned()))?,
        };
        if !item.definition.enabled {
            return Err(CatalogError::Disabled(agent_id.to_owned()));
        }
        Ok(item.clone())
    }
}
