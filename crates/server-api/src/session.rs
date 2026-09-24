use std::sync::Arc;

use serde_json::{Value, json};
use server_metadata::protocol::session::{EventsRequest, Heartbeat};
use server_metadata::service::session::SessionError;
use server_protocol::{ErrorCode, ServerMessage};

use crate::Shared;
use crate::connection::{ConnectionSubscriptions, MAX_SUBSCRIPTIONS};
use crate::outbound::{Outbound, QueueError};

pub(super) fn heartbeat(
    params: Value,
    subscriptions: &ConnectionSubscriptions,
) -> Result<(), ErrorCode> {
    let heartbeat: Heartbeat =
        serde_json::from_value(params).map_err(|_| ErrorCode::InvalidMessage)?;
    subscriptions
        .session
        .as_ref()
        .ok_or(ErrorCode::InvalidMessage)?
        .heartbeat(heartbeat)
        .map_err(map_error)
}

pub(super) fn subscribe(
    request_id: String,
    params: Value,
    state: &Shared,
    outbound: &Outbound,
    subscriptions: &mut ConnectionSubscriptions,
) -> Result<(), QueueError> {
    let prepared = (|| {
        let request: EventsRequest =
            serde_json::from_value(params).map_err(|_| ErrorCode::InvalidMessage)?;
        if subscriptions.len() >= MAX_SUBSCRIPTIONS {
            return Err(ErrorCode::ResourceExhausted);
        }
        if state.cancellation.is_cancelled() {
            return Err(ErrorCode::ServerDraining);
        }
        if request.events.iter().any(|event| {
            (event == "agent_attention_required" && state.agent_execution.is_none())
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
