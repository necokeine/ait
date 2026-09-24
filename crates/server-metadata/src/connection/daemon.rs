use crate::rpc::daemon;
use serde_json::Value;
use server_model::ErrorCode;

use crate::dispatch::State as Shared;

pub async fn dispatch(method: &str, params: Value, state: &Shared) -> Result<Value, ErrorCode> {
    if let Some(request) = daemon::lifecycle(method, &params)? {
        state.request_lifecycle(request.intent);
        return Ok(request.value);
    }
    let method = method.to_owned();
    let capabilities = state.info.implemented_capabilities.clone();
    let lifecycle = state.info().lifecycle;
    let events = state.session_events.clone();
    state
        .run(state.daemon.clone(), ErrorCode::DaemonIo, move |daemon| {
            let result = daemon::execute(daemon, &method, params, &capabilities, lifecycle)?;
            if matches!(
                method.as_str(),
                "daemon.config.set.request" | "daemon.config.reload.request"
            ) {
                let config = if method == "daemon.config.set.request" {
                    result["config"].clone()
                } else {
                    daemon::execute(
                        daemon,
                        "daemon.config.get.request",
                        serde_json::json!({}),
                        &capabilities,
                        lifecycle,
                    )?["config"]
                        .clone()
                };
                events.publish(
                    crate::protocol::session::SessionEventKind::DaemonConfig,
                    &serde_json::json!({"status":"daemon_config_changed","config":config}),
                );
            }
            Ok(result)
        })
        .await
}
