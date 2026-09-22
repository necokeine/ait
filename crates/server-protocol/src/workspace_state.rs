//! Workspace attention and archived-placement recovery payloads.

use serde::{Deserialize, Serialize};

/// Canonical Workspace state methods implemented by the independent server.
pub const CAPABILITIES: &[&str] = &[
    "workspace.clear_attention.request",
    "workspace.mark_unread.request",
    "workspace.recovery.inspect.request",
    "workspace.recovery.restore.request",
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

/// Recovery request shared by inspect and restore.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceRecoveryRequest {
    /// Archived Workspace identity.
    pub workspace_id: String,
}

/// Recovery action selected from durable placement and local filesystem state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceRecoveryAction {
    /// Reopen records because the exact directory still exists.
    Unarchive,
    /// Recreate a deleted managed worktree, then reopen records.
    Restore,
}

/// Stable unavailable reason from Paseo's Workspace recovery service.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceRecoveryUnavailableReason {
    /// No Workspace record exists.
    WorkspaceNotFound,
    /// The Workspace is already active.
    WorkspaceNotArchived,
    /// Its Project record no longer exists.
    ProjectNotFound,
    /// The source repository is missing.
    ProjectDirectoryMissing,
    /// A deleted non-worktree directory cannot be recreated.
    WorkspaceDirectoryMissing,
    /// The archived worktree lacks a branch.
    WorktreeBranchMissing,
}

/// Read-only Workspace recovery state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WorkspaceRecoveryState {
    /// Recovery can proceed.
    Recoverable {
        /// Workspace identity.
        #[serde(rename = "workspaceId")]
        workspace_id: String,
        /// Stored Workspace display name.
        #[serde(rename = "workspaceName")]
        workspace_name: String,
        /// Action restore will perform.
        action: WorkspaceRecoveryAction,
        /// Saved branch, or null.
        branch: Option<String>,
    },
    /// Recovery is unsafe or unnecessary.
    Unavailable {
        /// Workspace identity.
        #[serde(rename = "workspaceId")]
        workspace_id: String,
        /// Stable reason.
        reason: WorkspaceRecoveryUnavailableReason,
        /// User-facing explanation.
        message: String,
    },
}

/// Recovery inspection response payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WorkspaceRecoveryInspectResult {
    /// Current recoverability.
    pub state: WorkspaceRecoveryState,
}

/// Recovery mutation response payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceRecoveryRestoreResult {
    /// Requested Workspace identity.
    pub workspace_id: String,
    /// Whether recovery completed.
    pub accepted: bool,
    /// Inline error text.
    pub error: Option<String>,
}

#[cfg(test)]
mod tests;
