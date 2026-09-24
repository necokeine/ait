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
        state.directory.clone(),
        ErrorCode::RegistryIo,
        move |directory| {
            server_metadata::rpc::directory::execute(directory, &method, params).map_err(Into::into)
        },
    )
    .await
}
