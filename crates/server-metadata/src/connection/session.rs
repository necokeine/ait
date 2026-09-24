use std::sync::Arc;

use crate::protocol::session::{EventsRequest, Heartbeat};
use crate::service::session::SessionError;
use serde_json::{Value, json};
use server_model::{ErrorCode, ServerMessage};

use crate::connection::Connection;
use crate::dispatch::State as Shared;
use server_model::Context;
use server_model::outbound::QueueError;

/// Update connection presence from a validated heartbeat payload.
/// # Errors
/// Rejects malformed payloads, a missing connection or invalid presence state.
pub fn heartbeat(params: Value, subscriptions: &Connection) -> Result<(), ErrorCode> {
    let heartbeat: Heartbeat =
        serde_json::from_value(params).map_err(|_| ErrorCode::InvalidMessage)?;
    subscriptions
        .session
        .as_ref()
        .ok_or(ErrorCode::InvalidMessage)?
        .heartbeat(heartbeat)
        .map_err(map_error)
}

pub(crate) fn subscribe(
    context: Context<'_>,
    state: &Shared,
    subscriptions: &mut Connection,
) -> Result<(), QueueError> {
    let Context {
        request,
        outbound,
        available_subscriptions,
        ..
    } = context;
    let request_id = request.id;
    let params = request.params;
    let prepared = (|| {
        let request: EventsRequest =
            serde_json::from_value(params).map_err(|_| ErrorCode::InvalidMessage)?;
        if available_subscriptions == 0 {
            return Err(ErrorCode::ResourceExhausted);
        }
        if state.cancellation.is_cancelled() {
            return Err(ErrorCode::ServerDraining);
        }
        if request.events.iter().any(|event| {
            (event == "agent_attention_required" && !state.has_agent_execution)
                || (event == "status.daemon_config_changed" && state.daemon.is_none())
        }) {
            return Err(ErrorCode::UnsupportedCapability);
        }
        let sink = outbound.clone();
        subscriptions
            .session
            .as_ref()
            .ok_or(ErrorCode::InvalidMessage)?
            .subscribe(
                request,
                Arc::new(move |kind, params| {
                    sink.send(&ServerMessage::Event {
                        method: kind.method().to_owned(),
                        params,
                    })
                    .map_err(|_| SessionError::Closed)
                }),
            )
            .map_err(map_error)
    })();
    match prepared {
        Ok(subscription) => {
            let id = subscription.id().to_owned();
            outbound.send(&ServerMessage::Response {
                request_id,
                result: json!({"subscriptionId":id}),
            })?;
            if subscription.activate().is_err() {
                outbound.failure().cancel();
                return Err(QueueError::Full);
            }
            subscriptions.events.insert(id, subscription);
            Ok(())
        }
        Err(code) => outbound.send(&ServerMessage::Error {
            request_id: Some(request_id),
            code,
            message: code.message().to_owned(),
            retryable: code.retryable(),
        }),
    }
}

const fn map_error(error: SessionError) -> ErrorCode {
    match error {
        SessionError::Invalid => ErrorCode::InvalidMessage,
        SessionError::Unsupported => ErrorCode::UnsupportedCapability,
        SessionError::Closed => ErrorCode::ResourceExhausted,
    }
}
