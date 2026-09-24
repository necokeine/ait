use serde_json::Value;
use server_metadata::rpc::daemon;
use server_protocol::ErrorCode;

use crate::Shared;

pub(super) async fn dispatch(
    method: &str,
    params: Value,
    state: &Shared,
) -> Result<Value, ErrorCode> {
    if let Some(request) = daemon::lifecycle(method, &params)? {
        state.request_lifecycle(request.intent);
        return Ok(request.value);
    }
    let method = method.to_owned();
    let capabilities = state.info.implemented_capabilities.clone();
    let lifecycle = state.info().lifecycle;
    crate::jobs::run(
        state,
        state.daemon.clone(),
        ErrorCode::DaemonIo,
        move |daemon| {
            daemon::execute(daemon, &method, params, &capabilities, lifecycle).map_err(Into::into)
        },
    )
    .await
}
