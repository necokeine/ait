use std::{collections::HashMap, sync::RwLock};

use serde::{Deserialize, Serialize};

use crate::{CredentialRef, ProviderCapabilities, ProviderParameters};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
/// Configures one named provider-backed agent.
pub struct AgentDefinition {
    /// Stable identifier used to publish and resolve revisions.
    pub id: String,
    /// Human-readable display name.
    pub name: String,
    /// Provider adapter driver identifier.
    pub driver: String,
    /// Model identifier supplied to the provider.
    pub model: String,
    /// Optional provider endpoint override.
    pub endpoint: Option<String>,
    /// Optional reference resolved to a credential at invocation time.
    pub credential_ref: Option<CredentialRef>,
    /// Capabilities declared for this agent definition.
    pub capabilities: ProviderCapabilities,
    #[serde(default)]
    /// Default request parameters applied to invocations.
    pub default_parameters: ProviderParameters,
    /// Whether new runs may resolve this definition.
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
/// One immutable revision of an agent definition.
pub struct AgentRevision {
    /// Identifier of the versioned agent.
    pub agent_id: String,
    /// Monotonically increasing revision number.
    pub revision: u64,
    /// Definition stored at this revision.
    pub definition: AgentDefinition,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
/// Errors produced while publishing or resolving catalog entries.
pub enum CatalogError {
    #[error("agent id cannot be empty")]
    /// The submitted definition has a blank identifier.
    EmptyId,
    #[error("agent not found: {0}")]
    /// No revisions exist for the requested agent identifier.
    NotFound(String),
    #[error("agent revision not found: {agent_id}@{revision}")]
    /// The requested agent revision does not exist.
    RevisionNotFound {
        /// Identifier of the requested agent.
        agent_id: String,
        /// Missing revision number.
        revision: u64,
    },
    #[error("agent is disabled: {0}")]
    /// The resolved definition is disabled.
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
