//! Transport-independent workspace state request handling.

use chrono::{SecondsFormat, Utc};
use serde::Serialize;
use serde_json::Value;

use crate::protocol::workspace_state::{
    WorkspaceClearAttentionItem, WorkspaceClearAttentionRequest, WorkspaceClearAttentionResult,
    WorkspaceIdSelection, WorkspaceMarkUnreadRequest, WorkspaceMarkUnreadResult,
};
use crate::rpc::ErrorCode;
use crate::service::workspace_state::{WorkspaceAttentionBatch, WorkspaceState};

/// Completed Workspace state request.
#[derive(Debug)]
pub struct Dispatched {
    /// Response payload.
    pub value: Value,
    /// Optional Workspace event sent after the response.
    pub event: Option<Value>,
}

/// Decode and execute a business request.
///
/// # Errors
/// Returns stable business failures for invalid or unsuccessful requests.
pub fn execute(
    workspace_state: &mut WorkspaceState,
    method: &str,
    params: Value,
) -> Result<Dispatched, ErrorCode> {
    match method {
        "workspace.clear_attention.request" => clear_attention(workspace_state, decode(params)?),
        "workspace.mark_unread.request" => mark_unread(workspace_state, decode(params)?),
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
