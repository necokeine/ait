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
        state.workspace_automation.clone(),
        ErrorCode::RegistryIo,
        move |automation| {
            server_metadata::rpc::workspace_automation::execute(automation, &method, params)
                .map_err(Into::into)
        },
    )
    .await
}
