use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::{DomainError, DomainMetadata, ErrorCode, TimestampMs};

/// Stable identity of an Agent.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AgentId(String);

impl AgentId {
    /// Creates an externally assigned Agent identity.
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Returns the string representation.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A capability declared by an immutable Agent revision.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentCapability {
    /// Produces text content.
    Text,
    /// Accepts referenced files.
    FileInput,
    /// Emits structured data.
    StructuredOutput,
    /// Emits tool calls.
    ToolUse,
    /// May emit more than one tool call in a Message.
    ParallelToolUse,
    /// Supports continuation from a persisted checkpoint.
    CheckpointRecovery,
}

/// Default decision for a tool selected by an Agent.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolPermission {
    /// Execute without an approval gate.
    Allow,
    /// Pause until explicit approval is granted or denied.
    RequireApproval,
    /// Do not execute the tool.
    Deny,
}

/// Versioned tool policy used for deterministic Run behavior.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ToolPolicy {
    /// Permission used when no exact tool-name override exists.
    pub default: ToolPermission,
    /// Exact registered tool-name overrides in deterministic key order.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub tools: BTreeMap<String, ToolPermission>,
}

impl Default for ToolPolicy {
    fn default() -> Self {
        Self {
            default: ToolPermission::Deny,
            tools: BTreeMap::new(),
        }
    }
}

impl ToolPolicy {
    /// Resolves the effective permission for a registered tool name.
    #[must_use]
    pub fn permission_for(&self, tool_name: &str) -> ToolPermission {
        self.tools.get(tool_name).copied().unwrap_or(self.default)
    }
}

/// Mutable Agent catalog entry pointing to its latest immutable revision.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Agent {
    /// Agent identity.
    pub id: AgentId,
    /// Human-readable name.
    pub name: String,
    /// Latest available immutable configuration revision.
    pub config_revision: u64,
    /// Whether new Runs may select the Agent.
    pub enabled: bool,
    /// Creation time.
    pub created_at: TimestampMs,
    /// Last catalog update time.
    pub updated_at: TimestampMs,
}

impl Agent {
    /// Validates catalog-level invariants.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorCode::InvalidAgentConfiguration`] for an empty identity or
    /// name, a zero revision, or timestamps in the wrong order.
    pub fn validate(&self) -> Result<(), DomainError> {
        if self.id.as_str().is_empty()
            || self.name.trim().is_empty()
            || self.config_revision == 0
            || self.updated_at < self.created_at
        {
            return Err(DomainError::invariant(
                ErrorCode::InvalidAgentConfiguration,
                "agent identity, name, revision, or timestamps are invalid",
            ));
        }
        Ok(())
    }
}

/// Immutable, non-secret configuration of one Agent revision.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AgentRevision {
    /// Owning Agent.
    pub agent_id: AgentId,
    /// Monotonic Agent-local revision beginning at one.
    pub revision: u64,
    /// Adapter selection key, such as `codex`.
    pub driver_type: String,
    /// Name of a host-side connection block; never credential material.
    pub connection_name: String,
    /// Provider model identifier.
    pub model: String,
    /// Optional non-secret endpoint override.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
    /// Declared behavior supported by this revision.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub capabilities: BTreeSet<AgentCapability>,
    /// Provider-neutral and provider-specific generation parameters.
    #[serde(default)]
    pub default_parameters: DomainMetadata,
    /// Tool authorization policy fixed by this revision.
    #[serde(default)]
    pub tool_policy: ToolPolicy,
    /// SHA-256 digest of the canonical non-secret configuration.
    pub config_digest: String,
    /// Revision creation time.
    pub created_at: TimestampMs,
}

impl AgentRevision {
    /// Validates immutable revision fields and the SHA-256 digest format.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorCode::InvalidAgentConfiguration`] when required fields are
    /// empty, the revision is zero, or the digest is not lowercase hexadecimal.
    pub fn validate(&self) -> Result<(), DomainError> {
        if self.agent_id.as_str().is_empty()
            || self.revision == 0
            || self.driver_type.trim().is_empty()
            || self.connection_name.trim().is_empty()
            || self.model.trim().is_empty()
            || !is_sha256(&self.config_digest)
            || self.tool_policy.tools.keys().any(String::is_empty)
        {
            return Err(DomainError::invariant(
                ErrorCode::InvalidAgentConfiguration,
                "agent revision contains an invalid required field or digest",
            ));
        }
        Ok(())
    }

    /// Creates the fixed, non-secret snapshot embedded in a Run.
    #[must_use]
    pub fn snapshot(&self) -> AgentConfigSnapshot {
        AgentConfigSnapshot {
            agent_id: self.agent_id.clone(),
            revision: self.revision,
            driver_type: self.driver_type.clone(),
            connection_name: self.connection_name.clone(),
            model: self.model.clone(),
            endpoint: self.endpoint.clone(),
            capabilities: self.capabilities.clone(),
            default_parameters: self.default_parameters.clone(),
            tool_policy: self.tool_policy.clone(),
            config_digest: self.config_digest.clone(),
        }
    }
}

/// Immutable Agent revision data captured when a Run is created.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AgentConfigSnapshot {
    /// Selected Agent.
    pub agent_id: AgentId,
    /// Selected immutable revision.
    pub revision: u64,
    /// Adapter selection key.
    pub driver_type: String,
    /// Host-side non-secret connection name.
    pub connection_name: String,
    /// Provider model identifier.
    pub model: String,
    /// Optional non-secret endpoint override.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
    /// Fixed capability declaration.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub capabilities: BTreeSet<AgentCapability>,
    /// Fixed default parameters.
    #[serde(default)]
    pub default_parameters: DomainMetadata,
    /// Fixed tool policy.
    #[serde(default)]
    pub tool_policy: ToolPolicy,
    /// Configuration digest used to verify the snapshot.
    pub config_digest: String,
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn revision() -> AgentRevision {
        AgentRevision {
            agent_id: AgentId::new("agent-1"),
            revision: 2,
            driver_type: "codex".into(),
            connection_name: "default".into(),
            model: "gpt-5".into(),
            endpoint: None,
            capabilities: BTreeSet::from([AgentCapability::Text, AgentCapability::ToolUse]),
            default_parameters: DomainMetadata::default(),
            tool_policy: ToolPolicy::default(),
            config_digest: "a".repeat(64),
            created_at: TimestampMs(1),
        }
    }

    #[test]
    fn revision_snapshot_is_fixed_and_serializable() {
        let revision = revision();
        revision.validate().unwrap();
        let snapshot = revision.snapshot();
        let encoded = serde_json::to_string(&snapshot).unwrap();
        let decoded: AgentConfigSnapshot = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, snapshot);
        assert!(encoded.contains("\"tool_use\""));
    }

    #[test]
    fn invalid_digest_is_rejected() {
        let mut revision = revision();
        revision.config_digest = "secret".into();
        assert_eq!(
            revision.validate().unwrap_err().code,
            ErrorCode::InvalidAgentConfiguration
        );
    }
}
