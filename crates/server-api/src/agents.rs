use serde_json::Value;
use server_protocol::ErrorCode;

use crate::Shared;

pub(super) async fn dispatch(
    method: &str,
    params: Value,
    state: &Shared,
) -> Result<Value, ErrorCode> {
    let method = method.to_owned();
    crate::jobs::run(
        state,
        state.agents.clone(),
        ErrorCode::AgentIo,
        move |agents| {
            server_provider::rpc::agents::execute(agents, &method, params).map_err(Into::into)
        },
    )
    .await
}
