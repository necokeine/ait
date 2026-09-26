//! Provider-owned child identity; host Agents remain separate registration records.

use serde_json::Value;

/// A discovered native descendant and its verified immediate-parent link.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct NativeSubagent {
    /// Optional provider locator for children whose native transcript is not a root session.
    pub persistence: Option<server_domain::agent_runtime::AgentPersistenceHandle>,
    /// Native child identity.
    pub id: String,
    /// Native immediate parent, used to enforce root ancestry.
    pub parent_id: String,
    /// Native directory, used only after the parent relationship is validated.
    pub cwd: String,
    /// Paseo descriptor fields except the host parentAgentId and parentSubagentId.
    pub descriptor: Value,
}

/// Provider-owned child updates, isolated from the root timeline and foreground turn.
#[derive(Debug, Clone, PartialEq)]
pub enum SubagentEvent {
    /// Complete descriptor with a verified immediate-parent link.
    Upsert(NativeSubagent),
    /// A retry-stable incremental observation in the child's own timeline.
    Progress {
        /// Canonical provider child identity.
        id: String,
        /// Unique native observation identity.
        observation: String,
        /// Text delta or running tool snapshot.
        entry: crate::protocol::timeline::NativeItem,
    },
    /// A completed immutable child timeline item.
    Timeline {
        /// Canonical provider child identity.
        id: String,
        /// Completed source item.
        entry: crate::protocol::timeline::NativeItem,
    },
}
