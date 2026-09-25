//! Consumer-owned Workspace attention boundary without Agent record or Provider dependencies.

use std::fmt::Debug;

/// Workspace state operation failure.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum WorkspaceStateError {
    /// Workspace is missing or archived for an attention mutation.
    #[error("Workspace not found: {0}")]
    WorkspaceNotFound(String),
    /// No eligible finished root Agent exists.
    #[error("Workspace has no finished agent to mark unread: {0}")]
    NoFinishedAgent(String),
    /// The selected Agent changed after candidate selection.
    #[error("Agent is no longer finished and read: {0}")]
    AgentNoLongerFinished(String),
    /// Agent runtime persistence failed.
    #[error("Agent runtime registry failed")]
    AgentRegistry,
    /// Workspace registry persistence failed.
    #[error("Workspace registry failed")]
    WorkspaceRegistry,
}

/// Durable changes completed before an optional per-Workspace failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceAttentionChanges {
    /// Agent identities in the original registry order.
    pub cleared_agent_ids: Vec<String>,
    /// Failure after any preceding updates have committed.
    pub error: Option<WorkspaceStateError>,
}

/// One request's candidate snapshot, shared by all Workspaces in a clear-attention batch.
pub trait WorkspaceAttentionScan: Debug {
    /// Clear eligible attention using the captured candidates and return partial durable results.
    fn clear_attention(&self, workspace_id: &str, updated_at: &str) -> WorkspaceAttentionChanges;
}

/// Agent-owned attention operations consumed by Workspace services.
pub trait WorkspaceAttention: Debug + Send + Sync {
    /// Capture candidates once before processing a batch of Workspaces.
    ///
    /// # Errors
    /// Returns a categorized failure when Agent state cannot be read.
    fn scan(&self) -> Result<Box<dyn WorkspaceAttentionScan + '_>, WorkspaceStateError>;

    /// Mark the newest eligible finished root Agent in an already validated Workspace as unread.
    ///
    /// # Errors
    /// Returns a missing candidate, concurrent state change, or persistence failure.
    fn mark_unread(
        &self,
        workspace_id: &str,
        updated_at: &str,
    ) -> Result<String, WorkspaceStateError>;
}
