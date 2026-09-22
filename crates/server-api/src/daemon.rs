use serde_json::Value;
use server_application::daemon::{Daemon, DaemonError};
use server_protocol::daemon::{
    ConfigReloadResult, DaemonConfig, DaemonConfigResult, DaemonConfigSetRequest, DaemonStatus,
    DaemonUpdateResult, DiagnosticsResult, EmptyRequest, LifecycleResult, PairingOffer,
    RestartRequest,
};
use server_protocol::{ErrorCode, Lifecycle};

use crate::{LifecycleIntent, Shared};

pub(super) async fn dispatch(
    method: &str,
    params: Value,
    state: &Shared,
) -> Result<Value, ErrorCode> {
    match method {
        "server.restart.request" => {
            let request: RestartRequest = decode(params)?;
            let reason = request
                .reason
                .filter(|reason| !reason.trim().is_empty())
                .unwrap_or_else(|| "websocket_request".to_owned());
            let result = encode(LifecycleResult {
                status: "restart_requested".to_owned(),
                reason: Some(reason.clone()),
            })?;
            state.request_lifecycle(LifecycleIntent::Restart { reason });
            Ok(result)
        }
        "server.shutdown.request" => {
            let _: EmptyRequest = decode(params)?;
            let result = encode(LifecycleResult {
                status: "shutdown_requested".to_owned(),
                reason: None,
            })?;
            state.request_lifecycle(LifecycleIntent::Shutdown);
            Ok(result)
        }
        _ => {
            let method = method.to_owned();
            let capabilities = state.info.capabilities.clone();
            let lifecycle = state.info().lifecycle;
            crate::jobs::run(
                state,
                state.daemon.clone(),
                ErrorCode::DaemonIo,
                move |daemon| execute(daemon, &method, params, &capabilities, lifecycle),
            )
            .await
        }
    }
}

fn execute(
    daemon: &mut Daemon,
    method: &str,
    params: Value,
    capabilities: &[String],
    lifecycle: Lifecycle,
) -> Result<Value, ErrorCode> {
    match method {
        "daemon.get_status.request" => {
            let _: EmptyRequest = decode(params)?;
            let runtime = daemon.runtime();
            encode(DaemonStatus {
                server_id: runtime.server_id.clone(),
                version: runtime.version.clone(),
                pid: runtime.pid,
                node_path: runtime.executable.clone(),
                started_at: runtime.started_at.clone(),
                listen: Some(runtime.listen.clone()),
                relay: None,
                providers: Vec::new(),
            })
        }
        "daemon.get_pairing_offer.request" => {
            let _: EmptyRequest = decode(params)?;
            encode(PairingOffer {
                url: String::new(),
                qr: None,
                relay_enabled: false,
            })
        }
        "daemon.config.get.request" => {
            let _: EmptyRequest = decode(params)?;
            encode(DaemonConfigResult {
                config: config(daemon.get_config().map_err(error)?)?,
            })
        }
        "daemon.config.set.request" => {
            let request: DaemonConfigSetRequest = decode(params)?;
            request
                .config
                .validate()
                .map_err(|_| ErrorCode::InvalidMessage)?;
            let patch =
                serde_json::to_value(request.config).map_err(|_| ErrorCode::DaemonConfigInvalid)?;
            encode(DaemonConfigResult {
                config: config(daemon.set_config(&patch).map_err(error)?)?,
            })
        }
        "daemon.config.reload.request" => {
            let _: EmptyRequest = decode(params)?;
            let reload = daemon.reload_config().map_err(error)?;
            config(reload.config)?;
            encode(ConfigReloadResult {
                applied_paths: reload.applied_paths,
                restart_required_paths: reload.restart_required_paths,
                override_controlled_paths: reload.override_controlled_paths,
            })
        }
        "diagnostics.request" => {
            let _: EmptyRequest = decode(params)?;
            let lifecycle = match lifecycle {
                Lifecycle::Ready => "ready",
                Lifecycle::Draining => "draining",
            };
            encode(DiagnosticsResult {
                diagnostic: daemon.diagnostics(capabilities, lifecycle),
            })
        }
        "daemon.update.request" => {
            let _: EmptyRequest = decode(params)?;
            let result = daemon.update_result();
            encode(DaemonUpdateResult {
                success: result.success,
                error: result.error,
                previous_version: result.previous_version,
                new_version: result.new_version,
            })
        }
        _ => Err(ErrorCode::MethodNotFound),
    }
}

fn config(value: Value) -> Result<DaemonConfig, ErrorCode> {
    let config: DaemonConfig =
        serde_json::from_value(value).map_err(|_| ErrorCode::DaemonConfigInvalid)?;
    config
        .validate()
        .map_err(|_| ErrorCode::DaemonConfigInvalid)?;
    Ok(config)
}

fn decode<T: serde::de::DeserializeOwned>(params: Value) -> Result<T, ErrorCode> {
    serde_json::from_value(params).map_err(|_| ErrorCode::InvalidMessage)
}

fn encode(value: impl serde::Serialize) -> Result<Value, ErrorCode> {
    serde_json::to_value(value).map_err(|_| ErrorCode::DaemonIo)
}

fn error(error: DaemonError) -> ErrorCode {
    match error {
        DaemonError::InvalidConfig => ErrorCode::DaemonConfigInvalid,
        DaemonError::ConfigIo => ErrorCode::DaemonIo,
    }
}

#[cfg(test)]
mod tests;
