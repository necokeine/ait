//! Workspace attention validation and batch coordination over a consumer-owned port.

use crate::model::registry::PersistedWorkspaceRecord;
use crate::ports::registry::{RegistryError, WorkspaceRegistry};
use crate::ports::workspace_state::{WorkspaceAttention, WorkspaceAttentionScan};

pub use crate::ports::workspace_state::WorkspaceStateError;

/// Result for one Workspace in a clear-attention batch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceAttentionResult {
    /// Requested Workspace identity.
    pub workspace_id: String,
    /// Agents whose attention was durably cleared before this result completed.
    pub cleared_agent_ids: Vec<String>,
    /// Whether every eligible Agent in this Workspace was processed.
    pub success: bool,
    /// Inline error text for this Workspace.
    pub error: Option<String>,
}

/// Aggregate clear-attention result matching Paseo's batch semantics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceAttentionBatch {
    /// Flattened cleared Agent identities in request and registry order.
    pub cleared_agent_ids: Vec<String>,
    /// One result for every requested Workspace, including failures.
    pub results: Vec<WorkspaceAttentionResult>,
    /// True only when every Workspace succeeded.
    pub success: bool,
    /// Semicolon-separated errors, or none when the whole batch succeeded.
    pub error: Option<String>,
}

/// Coordinates Workspace attention without importing Agent records or Provider execution.
#[derive(Debug)]
pub struct WorkspaceState {
    attention: Box<dyn WorkspaceAttention>,
    workspaces: Box<dyn WorkspaceRegistry>,
}

impl WorkspaceState {
    /// Compose an Agent-owned attention adapter and the shared Workspace registry.
    #[must_use]
    pub fn new(
        attention: Box<dyn WorkspaceAttention>,
        workspaces: Box<dyn WorkspaceRegistry>,
    ) -> Self {
        Self {
            attention,
            workspaces,
        }
    }

    /// Clear non-permission attention for active Agents owned by each requested Workspace.
    ///
    /// Each Workspace is independent: a later failure does not hide earlier durable updates.
    #[must_use]
    pub fn clear_attention(
        &self,
        workspace_ids: &[String],
        updated_at: &str,
    ) -> WorkspaceAttentionBatch {
        let scan = match self.attention.scan() {
            Ok(scan) => scan,
            Err(error) => {
                return attention_batch(
                    workspace_ids
                        .iter()
                        .map(|workspace_id| WorkspaceAttentionResult {
                            workspace_id: workspace_id.clone(),
                            cleared_agent_ids: Vec::new(),
                            success: false,
                            error: Some(error.to_string()),
                        })
                        .collect(),
                );
            }
        };
        let results = workspace_ids
            .iter()
            .map(|workspace_id| {
                self.clear_workspace_attention(workspace_id, scan.as_ref(), updated_at)
            })
            .collect();
        attention_batch(results)
    }

    /// Mark the newest finished read root Agent in an active Workspace as unread.
    ///
    /// # Errors
    /// Returns Workspace validation failures or the Agent adapter's stable business error.
    pub fn mark_unread(
        &self,
        workspace_id: &str,
        updated_at: &str,
    ) -> Result<String, WorkspaceStateError> {
        self.require_active_workspace(workspace_id)?;
        self.attention.mark_unread(workspace_id, updated_at)
    }

    fn clear_workspace_attention(
        &self,
        workspace_id: &str,
        scan: &dyn WorkspaceAttentionScan,
        updated_at: &str,
    ) -> WorkspaceAttentionResult {
        let mut result = WorkspaceAttentionResult {
            workspace_id: workspace_id.to_owned(),
            cleared_agent_ids: Vec::new(),
            success: false,
            error: None,
        };
        if let Err(error) = self.require_active_workspace(workspace_id) {
            result.error = Some(error.to_string());
            return result;
        }
        let changes = scan.clear_attention(workspace_id, updated_at);
        result.cleared_agent_ids = changes.cleared_agent_ids;
        result.success = changes.error.is_none();
        result.error = changes.error.map(|error| error.to_string());
        result
    }

    fn require_active_workspace(
        &self,
        workspace_id: &str,
    ) -> Result<PersistedWorkspaceRecord, WorkspaceStateError> {
        let workspace = self
            .workspaces
            .get(workspace_id)
            .map_err(map_workspace_error)?
            .ok_or_else(|| WorkspaceStateError::WorkspaceNotFound(workspace_id.to_owned()))?;
        if is_archived(workspace.archived_at.as_deref()) {
            return Err(WorkspaceStateError::WorkspaceNotFound(
                workspace_id.to_owned(),
            ));
        }
        Ok(workspace)
    }
}

fn attention_batch(results: Vec<WorkspaceAttentionResult>) -> WorkspaceAttentionBatch {
    let cleared_agent_ids = results
        .iter()
        .flat_map(|result| result.cleared_agent_ids.iter().cloned())
        .collect();
    let errors = results
        .iter()
        .filter_map(|result| {
            (!result.success)
                .then_some(result.error.as_deref())
                .flatten()
        })
        .collect::<Vec<_>>();
    WorkspaceAttentionBatch {
        cleared_agent_ids,
        success: errors.is_empty(),
        error: (!errors.is_empty()).then(|| errors.join("; ")),
        results,
    }
}

fn is_archived(archived_at: Option<&str>) -> bool {
    archived_at.is_some_and(|value| !value.is_empty())
}

const fn map_workspace_error(_: RegistryError) -> WorkspaceStateError {
    WorkspaceStateError::WorkspaceRegistry
}

#[cfg(test)]
mod tests;
