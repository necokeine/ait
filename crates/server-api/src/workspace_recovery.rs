use serde_json::Value;
use server_filesystem::rpc::workspace_recovery::Dispatched;
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
        state.workspace_recovery.clone(),
        ErrorCode::RegistryIo,
        move |service| {
            server_filesystem::rpc::workspace_recovery::execute(service, &method, params)
                .map_err(Into::into)
        },
    )
    .await
}
