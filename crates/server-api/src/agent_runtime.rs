use serde_json::Value;
use server_protocol::ErrorCode;

use crate::Shared;

pub(super) async fn dispatch(
    method: &str,
    params: Value,
    state: &Shared,
) -> Result<Value, ErrorCode> {
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
