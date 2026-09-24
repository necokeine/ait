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
        state.forge.clone(),
        ErrorCode::ProjectIo,
        move |forge| {
            server_filesystem::rpc::forge::execute(forge, &method, params).map_err(Into::into)
        },
    )
    .await
}
