use serde_json::Value;
use server_protocol::ErrorCode;

use crate::Shared;

pub(super) async fn dispatch(
    method: &str,
    mut params: Value,
    state: &Shared,
) -> Result<Value, ErrorCode> {
    let terminal_ids = if method == "agent.items.close.request" {
        let request: server_provider::protocol::agent_lifecycle::AgentItemsCloseRequest =
            serde_json::from_value(params.clone()).map_err(|_| ErrorCode::InvalidMessage)?;
        if !request.terminal_ids.is_empty() && state.terminals.is_none() {
            return Err(ErrorCode::NotImplemented);
        }
        params["terminalIds"] = serde_json::json!([]);
        request.terminal_ids
    } else {
        Vec::new()
    };
    let mut result = dispatch_agents(method, params, state).await?;
    if !terminal_ids.is_empty() {
        result["terminals"] = crate::terminal::run(state, move |terminals| {
            Ok(serde_json::json!(
                terminal_ids
                    .into_iter()
                    .map(|id| {
                        let success = terminals.kill(&id).is_ok();
                        serde_json::json!({"terminalId":id,"success":success})
                    })
                    .collect::<Vec<_>>()
            ))
        })
        .await?;
    }
    Ok(result)
}

async fn dispatch_agents(method: &str, params: Value, state: &Shared) -> Result<Value, ErrorCode> {
    if state.agent_execution.is_some() {
        return crate::agent_execution::dispatch(method, params, state).await;
    }
    let method = method.to_owned();
    crate::jobs::run(
        state,
        state.agent_runtime.clone(),
        ErrorCode::AgentIo,
        move |directory| {
            server_provider::rpc::agent_runtime::execute(directory, &method, params)
                .map_err(Into::into)
        },
    )
    .await
}
