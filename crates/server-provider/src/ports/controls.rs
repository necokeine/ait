//! Provider-owned child identity; host Agents remain separate registration records.

use serde_json::Value;

/// A discovered native descendant and its verified immediate-parent link.
#[derive(Debug, Clone)]
pub struct NativeSubagent {
    /// Native child identity.
    pub id: String,
    /// Native immediate parent, used to enforce root ancestry.
    pub parent_id: String,
    /// Native directory, used only after the parent relationship is validated.
    pub cwd: String,
    /// Paseo descriptor fields except the host parentAgentId and parentSubagentId.
    pub descriptor: Value,
}
