use chrono::{SecondsFormat, Utc};
use serde::Serialize;
use serde_json::Value;
use server_application::workspace_state::{
    WorkspaceAttentionBatch, WorkspaceRecoveryAction as ApplicationRecoveryAction,
    WorkspaceRecoveryState as ApplicationRecoveryState,
    WorkspaceRecoveryUnavailableReason as ApplicationUnavailableReason, WorkspaceState,
};
use server_protocol::ErrorCode;
use server_protocol::workspace_state::{
    WorkspaceClearAttentionItem, WorkspaceClearAttentionRequest, WorkspaceClearAttentionResult,
    WorkspaceIdSelection, WorkspaceMarkUnreadRequest, WorkspaceMarkUnreadResult,
    WorkspaceRecoveryAction, WorkspaceRecoveryInspectResult, WorkspaceRecoveryRequest,
    WorkspaceRecoveryRestoreResult, WorkspaceRecoveryState, WorkspaceRecoveryUnavailableReason,
};

use crate::Shared;

/// Completed Workspace state dispatch plus an optional post-response Workspace update.
pub(super) struct Dispatched {
    pub(super) value: Value,
    pub(super) event: Option<Value>,
}

pub(super) async fn dispatch(
    method: &str,
    params: Value,
    state: &Shared,
) -> Result<Dispatched, ErrorCode> {
    let method = method.to_owned();
    crate::jobs::run(
        state,
        state.workspace_state.clone(),
        ErrorCode::RegistryIo,
        move |workspace_state| execute(workspace_state, &method, params),
    )
    .await
}

fn execute(
    workspace_state: &mut WorkspaceState,
    method: &str,
    params: Value,
) -> Result<Dispatched, ErrorCode> {
    match method {
        "workspace.clear_attention.request" => clear_attention(workspace_state, decode(params)?),
        "workspace.mark_unread.request" => mark_unread(workspace_state, decode(params)?),
        "workspace.recovery.inspect.request" => recovery_inspect(workspace_state, &decode(params)?),
        "workspace.recovery.restore.request" => recovery_restore(workspace_state, decode(params)?),
        _ => Err(ErrorCode::MethodNotFound),
    }
}

fn clear_attention(
    workspace_state: &WorkspaceState,
    request: WorkspaceClearAttentionRequest,
) -> Result<Dispatched, ErrorCode> {
    let ids = match &request.workspace_id {
        WorkspaceIdSelection::One(workspace_id) => vec![workspace_id.clone()],
        WorkspaceIdSelection::Many(workspace_ids) => workspace_ids.clone(),
    };
    let result = workspace_state.clear_attention(&ids, &timestamp());
    value(clear_attention_result(request.workspace_id, result))
}

fn mark_unread(
    workspace_state: &WorkspaceState,
    request: WorkspaceMarkUnreadRequest,
) -> Result<Dispatched, ErrorCode> {
    let result = match workspace_state.mark_unread(&request.workspace_id, &timestamp()) {
        Ok(agent_id) => WorkspaceMarkUnreadResult {
            workspace_id: request.workspace_id,
            marked_agent_id: Some(agent_id),
            success: true,
            error: None,
        },
        Err(error) => WorkspaceMarkUnreadResult {
            workspace_id: request.workspace_id,
            marked_agent_id: None,
            success: false,
            error: Some(error.to_string()),
        },
    };
    value(result)
}

fn recovery_inspect(
    workspace_state: &WorkspaceState,
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
    workspace_state: &WorkspaceState,
    request: WorkspaceRecoveryRequest,
) -> Result<Dispatched, ErrorCode> {
    match workspace_state.restore(&request.workspace_id, &timestamp()) {
        Ok(recovered) => {
            let descriptor = crate::directory::workspace_descriptor(
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

fn clear_attention_result(
    workspace_id: WorkspaceIdSelection,
    result: WorkspaceAttentionBatch,
) -> WorkspaceClearAttentionResult {
    WorkspaceClearAttentionResult {
        workspace_id,
        cleared_agent_ids: result.cleared_agent_ids,
        results: result
            .results
            .into_iter()
            .map(|item| WorkspaceClearAttentionItem {
                workspace_id: item.workspace_id,
                cleared_agent_ids: item.cleared_agent_ids,
                success: item.success,
                error: item.error,
            })
            .collect(),
        success: result.success,
        error: result.error,
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
