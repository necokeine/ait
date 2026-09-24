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
        state.github_projects.clone(),
        ErrorCode::RegistryIo,
        move |service| {
            server_filesystem::rpc::github_projects::execute(service, &method, params)
                .map_err(Into::into)
        },
    )
    .await
}
