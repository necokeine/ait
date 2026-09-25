//! Archived Workspace recovery messages.
use serde::{Deserialize, Serialize};
/// Canonical recovery methods implemented by the filesystem service.
pub const CAPABILITIES: &[&str] = &[
    "workspace.recovery.inspect.request",
    "workspace.recovery.restore.request",
];
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
