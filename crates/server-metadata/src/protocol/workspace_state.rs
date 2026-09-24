//! Workspace Agent attention payloads.

use serde::{Deserialize, Serialize};

/// Canonical Workspace state methods implemented by the independent server.
pub const CAPABILITIES: &[&str] = &[
    "workspace.clear_attention.request",
    "workspace.mark_unread.request",
];

/// One or several Workspace identities accepted by clear-attention.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(untagged)]
pub enum WorkspaceIdSelection {
    /// One Workspace identity.
    One(String),
    /// Several independently processed Workspace identities.
    Many(Vec<String>),
}

/// Clear non-permission Agent attention for one or several Workspaces.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceClearAttentionRequest {
    /// Workspace identity or batch.
    pub workspace_id: WorkspaceIdSelection,
}

/// Per-Workspace clear-attention result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceClearAttentionItem {
    /// Requested Workspace identity.
    pub workspace_id: String,
    /// Agents whose attention was cleared.
    pub cleared_agent_ids: Vec<String>,
    /// Whether this Workspace completed without an error.
    pub success: bool,
    /// Inline error text.
    pub error: Option<String>,
}

/// Aggregate clear-attention response payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceClearAttentionResult {
    /// Original singular or batch selection.
    pub workspace_id: WorkspaceIdSelection,
    /// Flattened cleared Agent identities.
    pub cleared_agent_ids: Vec<String>,
    /// One result per requested Workspace.
    pub results: Vec<WorkspaceClearAttentionItem>,
    /// True only when every Workspace succeeded.
    pub success: bool,
    /// Aggregate inline error text.
    pub error: Option<String>,
}

/// Mark the newest finished root Agent in a Workspace as unread.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceMarkUnreadRequest {
    /// Active Workspace identity.
    pub workspace_id: String,
}

/// Mark-unread response payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceMarkUnreadResult {
    /// Requested Workspace identity.
    pub workspace_id: String,
    /// Agent marked unread, or null on rejection.
    pub marked_agent_id: Option<String>,
    /// Whether the mutation completed.
    pub success: bool,
    /// Inline error text.
    pub error: Option<String>,
}

#[cfg(test)]
mod tests;
