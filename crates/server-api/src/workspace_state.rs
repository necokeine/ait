use serde_json::Value;
use server_metadata::rpc::workspace_state::Dispatched;
use server_protocol::ErrorCode;

use crate::Shared;

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
        move |workspace_state| {
            server_metadata::rpc::workspace_state::execute(workspace_state, &method, params)
                .map_err(Into::into)
        },
    )
    .await
}
