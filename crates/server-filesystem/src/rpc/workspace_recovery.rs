//! Workspace recovery dispatch and post-response update projection.
use chrono::{SecondsFormat, Utc};
use serde::Serialize;
use serde_json::Value;

use crate::protocol::workspace_recovery::{
    WorkspaceRecoveryAction, WorkspaceRecoveryInspectResult, WorkspaceRecoveryRequest,
    WorkspaceRecoveryRestoreResult, WorkspaceRecoveryState, WorkspaceRecoveryUnavailableReason,
};
use crate::rpc::ErrorCode;
use crate::service::workspace_recovery::{
    WorkspaceRecovery, WorkspaceRecoveryAction as ApplicationRecoveryAction,
    WorkspaceRecoveryState as ApplicationRecoveryState,
    WorkspaceRecoveryUnavailableReason as ApplicationUnavailableReason,
};
/// Recovery result and update to publish after its response.
pub struct Dispatched {
    /// Serialized response.
    pub value: Value,
    /// Optional Workspace update.
    pub event: Option<Value>,
}
/// Execute recovery inspection or restoration.
///
/// # Errors
/// Rejects invalid input, unknown methods, or registry failures.
pub fn execute(
    workspace_state: &WorkspaceRecovery,
    method: &str,
    params: Value,
) -> Result<Dispatched, ErrorCode> {
    match method {
        "workspace.recovery.inspect.request" => recovery_inspect(workspace_state, &decode(params)?),
        "workspace.recovery.restore.request" => recovery_restore(workspace_state, decode(params)?),
        _ => Err(ErrorCode::MethodNotFound),
    }
}
fn recovery_inspect(
    workspace_state: &WorkspaceRecovery,
    request: &WorkspaceRecoveryRequest,
) -> Result<Dispatched, ErrorCode> {
    let state = workspace_state
        .inspect_recovery(&request.workspace_id)
        .map_err(|_| ErrorCode::RegistryIo)?;
    value(WorkspaceRecoveryInspectResult {
        state: recovery_state(state),
    })
}

fn recovery_restore(
    workspace_state: &WorkspaceRecovery,
    request: WorkspaceRecoveryRequest,
) -> Result<Dispatched, ErrorCode> {
    match workspace_state.restore(&request.workspace_id, &timestamp()) {
        Ok(recovered) => {
            let descriptor = server_metadata::rpc::directory::workspace_descriptor(
                &recovered.workspace,
                Some(&recovered.project),
            );
            dispatched(
                WorkspaceRecoveryRestoreResult {
                    workspace_id: request.workspace_id,
                    accepted: true,
                    error: None,
                },
                Some(serde_json::json!({
                    "kind":"upsert",
                    "workspace":descriptor,
                })),
            )
        }
        Err(error) => value(WorkspaceRecoveryRestoreResult {
            workspace_id: request.workspace_id,
            accepted: false,
            error: Some(error.to_string()),
        }),
    }
}

fn recovery_state(state: ApplicationRecoveryState) -> WorkspaceRecoveryState {
    match state {
        ApplicationRecoveryState::Recoverable {
            workspace_id,
            workspace_name,
            action,
            branch,
        } => WorkspaceRecoveryState::Recoverable {
            workspace_id,
            workspace_name,
            action: match action {
                ApplicationRecoveryAction::Unarchive => WorkspaceRecoveryAction::Unarchive,
                ApplicationRecoveryAction::Restore => WorkspaceRecoveryAction::Restore,
            },
            branch,
        },
        ApplicationRecoveryState::Unavailable {
            workspace_id,
            reason,
            message,
        } => WorkspaceRecoveryState::Unavailable {
            workspace_id,
            reason: match reason {
                ApplicationUnavailableReason::WorkspaceNotFound => {
                    WorkspaceRecoveryUnavailableReason::WorkspaceNotFound
                }
                ApplicationUnavailableReason::WorkspaceNotArchived => {
                    WorkspaceRecoveryUnavailableReason::WorkspaceNotArchived
                }
                ApplicationUnavailableReason::ProjectNotFound => {
                    WorkspaceRecoveryUnavailableReason::ProjectNotFound
                }
                ApplicationUnavailableReason::ProjectDirectoryMissing => {
                    WorkspaceRecoveryUnavailableReason::ProjectDirectoryMissing
                }
                ApplicationUnavailableReason::WorkspaceDirectoryMissing => {
                    WorkspaceRecoveryUnavailableReason::WorkspaceDirectoryMissing
                }
                ApplicationUnavailableReason::WorktreeBranchMissing => {
                    WorkspaceRecoveryUnavailableReason::WorktreeBranchMissing
                }
            },
            message,
        },
    }
}

fn decode<T: serde::de::DeserializeOwned>(value: Value) -> Result<T, ErrorCode> {
    serde_json::from_value(value).map_err(|_| ErrorCode::InvalidMessage)
}

fn value(value: impl Serialize) -> Result<Dispatched, ErrorCode> {
    dispatched(value, None)
}

fn dispatched(value: impl Serialize, event: Option<Value>) -> Result<Dispatched, ErrorCode> {
    Ok(Dispatched {
        value: serde_json::to_value(value).map_err(|_| ErrorCode::RegistryIo)?,
        event,
    })
}

fn timestamp() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

#[cfg(test)]
mod tests;
