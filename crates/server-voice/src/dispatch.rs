//! Speech request entry point; the API owns only physical transport and negotiation.

use std::sync::Arc;

use server_model::{Context, ErrorCode, Runtime, outbound::QueueError};

use crate::{capabilities::Group, connection::Connection, service::Speech};

/// Concrete services and shared runtime used by speech request and event handlers.
#[derive(Debug)]
pub struct State {
    /// Common server task/admission resources.
    pub runtime: Arc<Runtime>,
    /// Optional installed speech service; its backend configuration may independently be disabled.
    pub speech: Option<Speech>,
}

/// Dispatch an admitted request to the connection's speech state.
/// # Errors
/// Returns encoding or queue errors; business errors are delivered in the correlated response.
pub async fn dispatch(
    group: Group,
    mut context: Context<'_>,
    state: &State,
    connection: &mut Connection,
) -> Result<(), QueueError> {
    let Group::Voice = group;
    let stopped = context.request.method == "voice.abort.request"
        || context.request.method == "voice.mode.set.request"
            && context.request.params["enabled"] == false;
    let result = match &state.speech {
        Some(speech) if !state.runtime.cancellation.is_cancelled() => connection
            .request(
                &context.request.method,
                std::mem::take(&mut context.request.params),
                speech,
            )
            .await
            .map_err(Into::into),
        Some(_) => Err(ErrorCode::ServerDraining),
        None => Err(ErrorCode::UnsupportedCapability),
    };
    let notify = stopped && result.is_ok();
    let outbound = context.outbound;
    context.respond(result)?;
    if notify {
        outbound.send(&server_model::ServerMessage::Event {
            method: "voice.input.state".to_owned(),
            params: serde_json::json!({"isSpeaking":false}),
        })?;
    }
    Ok(())
}
