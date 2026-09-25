use serde_json::Value;
use server_model::ErrorCode;

use crate::dispatch::State as Shared;

pub async fn dispatch(method: &str, mut params: Value, state: &Shared) -> Result<Reply, ErrorCode> {
    let terminal_ids = if method == "agent.items.close.request" {
        let request: crate::protocol::agent_lifecycle::AgentItemsCloseRequest =
            serde_json::from_value(params.clone()).map_err(|_| ErrorCode::InvalidMessage)?;
        if !request.terminal_ids.is_empty() && !state.has_terminals {
            return Err(ErrorCode::NotImplemented);
        }
        params["terminalIds"] = serde_json::json!([]);
        request.terminal_ids
    } else {
        Vec::new()
    };
    let value = dispatch_agents(method, params, state).await?;
    Ok(Reply {
        value,
        terminals: terminal_ids,
    })
}

pub(super) struct Reply {
    pub(super) value: Value,
    pub(super) terminals: Vec<String>,
}

async fn dispatch_agents(method: &str, params: Value, state: &Shared) -> Result<Value, ErrorCode> {
    if state.agent_execution.is_some() {
        return super::agent_execution::dispatch(method, params, state).await;
    }
    let method = method.to_owned();
    state
        .run(
            state.agent_runtime.clone(),
            ErrorCode::AgentIo,
            move |directory| {
                crate::rpc::agent_runtime::execute(directory, &method, params).map_err(Into::into)
            },
        )
        .await
}
