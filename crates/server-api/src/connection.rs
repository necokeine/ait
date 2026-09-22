use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::Duration;

use axum::extract::ws::{CloseFrame, Message, WebSocket, close_code};
use futures_util::{SinkExt, StreamExt, stream::SplitStream};
use serde_json::{Value, json};
use server_application::workspace_labels::WorkspaceLabelSubscription;
use server_protocol::subscription::{SubscriptionReleaseRequest, SubscriptionReleaseResult};
use server_protocol::{ClientMessage, ErrorCode, Lifecycle, ServerMessage, valid_id};
use tokio::time::timeout;
use uuid::Uuid;

use crate::Shared;
use crate::checkout::CheckoutDiffSubscription;
use crate::outbound::{Outbound, QueueError};

const HELLO_TIMEOUT: Duration = Duration::from_secs(10);
const WRITE_TIMEOUT: Duration = Duration::from_secs(5);
const CLOSE_TIMEOUT: Duration = Duration::from_secs(1);
const MAX_SUBSCRIPTIONS: usize = 16;

type RouteResult = (
    Result<Value, ErrorCode>,
    Option<crate::workspace_labels::PendingSubscription>,
    Option<crate::checkout::PendingSubscription>,
    Option<Value>,
);

#[derive(Default)]
struct ConnectionSubscriptions {
    diffs: BTreeMap<String, CheckoutDiffSubscription>,
    status: BTreeSet<String>,
    labels: BTreeMap<String, WorkspaceLabelSubscription>,
}

impl ConnectionSubscriptions {
    fn len(&self) -> usize {
        self.status
            .len()
            .saturating_add(self.labels.len())
            .saturating_add(self.diffs.len())
    }
}

pub(super) async fn serve(socket: WebSocket, state: Arc<Shared>) {
    let (mut sink, stream) = socket.split();
    let (outbound, mut receiver) = Outbound::new();
    let outbound_failure = outbound.failure();
    let stopped = tokio_util::sync::CancellationToken::new();
    let writer = async {
        loop {
            let queued = tokio::select! {
                biased;
                () = stopped.cancelled() => break,
                queued = receiver.recv() => match queued { Some(queued) => queued, None => break },
            };
            if !matches!(
                timeout(WRITE_TIMEOUT, sink.send(Message::Text(queued.text.into()))).await,
                Ok(Ok(()))
            ) {
                break;
            }
        }
        stopped.cancel();
        let _ = timeout(CLOSE_TIMEOUT, async {
            // Flush already bounded diagnostic/error/status messages before closing.
            while let Ok(queued) = receiver.try_recv() {
                sink.send(Message::Text(queued.text.into())).await?;
            }
            sink.send(Message::Close(Some(CloseFrame {
                code: close_code::AWAY,
                reason: "connection closing".into(),
            })))
            .await
        })
        .await;
    };
    let reader = async {
        tokio::select! {
            () = stopped.cancelled() => {},
            () = outbound_failure.cancelled() => {},
            _ = read(stream, &state, &outbound) => {},
        }
        stopped.cancel();
    };
    // Both halves are scoped to this tracked upgrade; neither can outlive its connection.
    tokio::join!(writer, reader);
}

async fn read(
    mut stream: SplitStream<WebSocket>,
    state: &Shared,
    outbound: &Outbound,
) -> Result<(), QueueError> {
    let hello = tokio::select! {
        () = state.cancellation.cancelled() => return Ok(()),
        result = timeout(HELLO_TIMEOUT, receive(&mut stream)) => match result {
            Ok(Some(Ok(ClientMessage::Hello(hello)))) => hello,
            _ => return error(outbound, None, ErrorCode::InvalidMessage),
        },
    };
    let capabilities = match hello.negotiate_available(&state.info.capabilities) {
        Ok(capabilities) => capabilities,
        Err(code) => return error(outbound, None, code),
    };
    outbound.send(&ServerMessage::ServerInfo {
        info: state.info(),
        connection_id: Uuid::new_v4().to_string(),
        negotiated_capabilities: capabilities.clone(),
    })?;
    let mut subscriptions = ConnectionSubscriptions::default();
    loop {
        let message = tokio::select! {
            biased;
            () = state.cancellation.cancelled() => {
                for subscription_id in subscriptions.status {
                    outbound.send(&ServerMessage::Status { subscription_id, lifecycle: Lifecycle::Draining })?;
                }
                return Ok(());
            },
            message = receive(&mut stream) => message,
        };
        match message {
            Some(Ok(ClientMessage::Request {
                request_id,
                method,
                params,
            })) if valid_id(&request_id) && valid_id(&method) => {
                process_request(
                    request_id,
                    method,
                    params,
                    state,
                    outbound,
                    &capabilities,
                    &mut subscriptions,
                )
                .await?;
            }
            None => return Ok(()),
            Some(Ok(ClientMessage::Hello(_) | ClientMessage::Request { .. }) | Err(_)) => {
                return error(outbound, None, ErrorCode::InvalidMessage);
            }
        }
    }
}

async fn process_request(
    request_id: String,
    method: String,
    params: Value,
    state: &Shared,
    outbound: &Outbound,
    capabilities: &[String],
    subscriptions: &mut ConnectionSubscriptions,
) -> Result<(), QueueError> {
    let (result, pending_label_subscription, pending_diff_subscription, workspace_event) =
        route_request(
            &method,
            params,
            state,
            outbound,
            capabilities,
            subscriptions,
        )
        .await;
    let value = match result {
        Ok(value) => value,
        Err(code) => return error(outbound, Some(request_id), code),
    };
    let status_subscription_id = (method == "server.status.subscribe")
        .then(|| {
            value
                .get("subscription_id")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .flatten();
    outbound.send(&ServerMessage::Response {
        request_id,
        result: value,
    })?;
    if let Some(pending) = pending_label_subscription {
        let (subscription_id, subscription) = pending.activate()?;
        subscriptions.labels.insert(subscription_id, subscription);
    }
    if let Some(pending) = pending_diff_subscription {
        let (subscription_id, subscription) = pending.activate();
        subscriptions.diffs.insert(subscription_id, subscription);
    }
    if let Some(subscription_id) = status_subscription_id {
        outbound.send(&ServerMessage::Status {
            subscription_id,
            lifecycle: state.info().lifecycle,
        })?;
    }
    if let Some(params) = workspace_event {
        outbound.send(&ServerMessage::Event {
            method: "workspace.update".to_owned(),
            params,
        })?;
    }
    Ok(())
}

async fn route_request(
    method: &str,
    params: Value,
    state: &Shared,
    outbound: &Outbound,
    capabilities: &[String],
    subscriptions: &mut ConnectionSubscriptions,
) -> RouteResult {
    let supported = capabilities.iter().any(|capability| capability == method);
    let result = if server_protocol::project_lease::CAPABILITIES.contains(&method) {
        if supported {
            crate::projects::dispatch(method, params, state).await
        } else {
            Err(ErrorCode::UnsupportedCapability)
        }
    } else if server_protocol::agent::CAPABILITIES.contains(&method) {
        if supported {
            crate::agents::dispatch(method, params, state).await
        } else {
            Err(ErrorCode::UnsupportedCapability)
        }
    } else if server_protocol::agent_lifecycle::CAPABILITIES.contains(&method) {
        if supported {
            crate::agent_runtime::dispatch(method, params, state).await
        } else {
            Err(ErrorCode::UnsupportedCapability)
        }
    } else if server_protocol::directory::CAPABILITIES.contains(&method)
        || server_protocol::project_config::CAPABILITIES.contains(&method)
        || server_protocol::project_icon::CAPABILITIES.contains(&method)
    {
        if supported {
            crate::directory::dispatch(method, params, state).await
        } else {
            Err(ErrorCode::UnsupportedCapability)
        }
    } else if server_protocol::daemon::CAPABILITIES.contains(&method) {
        if supported {
            crate::daemon::dispatch(method, params, state).await
        } else {
            Err(ErrorCode::UnsupportedCapability)
        }
    } else if server_protocol::workspace_labels::CAPABILITIES.contains(&method) {
        if !supported {
            return (Err(ErrorCode::UnsupportedCapability), None, None, None);
        }
        if method == "workspace.label.list.request"
            && params
                .get("subscribe")
                .is_some_and(|subscribe| !subscribe.is_null())
            && subscriptions.len() >= MAX_SUBSCRIPTIONS
        {
            return (Err(ErrorCode::ResourceExhausted), None, None, None);
        }
        return match crate::workspace_labels::dispatch(method, params, state, outbound.clone())
            .await
        {
            Ok(dispatched) => (Ok(dispatched.value), dispatched.subscription, None, None),
            Err(error) => (Err(error), None, None, None),
        };
    } else if server_protocol::checkout::CAPABILITIES.contains(&method) {
        return route_checkout(method, params, state, outbound, supported, subscriptions).await;
    } else if server_protocol::worktrees::CAPABILITIES.contains(&method) {
        if !supported {
            return (Err(ErrorCode::UnsupportedCapability), None, None, None);
        }
        return match crate::worktrees::dispatch(method, params, state).await {
            Ok(dispatched) => (Ok(dispatched.value), None, None, dispatched.event),
            Err(error) => (Err(error), None, None, None),
        };
    } else if server_protocol::workspace_automation::CAPABILITIES.contains(&method) {
        if supported {
            crate::workspace_automation::dispatch(method, params, state).await
        } else {
            Err(ErrorCode::UnsupportedCapability)
        }
    } else if server_protocol::workspace_state::CAPABILITIES.contains(&method) {
        if !supported {
            return (Err(ErrorCode::UnsupportedCapability), None, None, None);
        }
        return match crate::workspace_state::dispatch(method, params, state).await {
            Ok(dispatched) => (Ok(dispatched.value), None, None, dispatched.event),
            Err(error) => (Err(error), None, None, None),
        };
    } else {
        dispatch(method, &params, state, capabilities, subscriptions)
    };
    (result, None, None, None)
}

async fn route_checkout(
    method: &str,
    params: Value,
    state: &Shared,
    outbound: &Outbound,
    supported: bool,
    subscriptions: &mut ConnectionSubscriptions,
) -> RouteResult {
    if !supported {
        return (Err(ErrorCode::UnsupportedCapability), None, None, None);
    }
    if method == "checkout.diff.unsubscribe.request" {
        let request = serde_json::from_value::<
            server_protocol::checkout::CheckoutDiffUnsubscribeRequest,
        >(params);
        let request = match request {
            Ok(request) if valid_id(&request.subscription_id) => request,
            _ => return (Err(ErrorCode::InvalidMessage), None, None, None),
        };
        if subscriptions
            .diffs
            .remove(&request.subscription_id)
            .is_none()
        {
            return (Err(ErrorCode::SubscriptionNotFound), None, None, None);
        }
        return (
            Ok(json!({"subscriptionId":request.subscription_id})),
            None,
            None,
            None,
        );
    }
    if method == "checkout.diff.subscribe.request" {
        let replacement = params
            .get("subscriptionId")
            .and_then(Value::as_str)
            .is_some_and(|id| subscriptions.diffs.contains_key(id));
        if subscriptions.len() >= MAX_SUBSCRIPTIONS && !replacement {
            return (Err(ErrorCode::ResourceExhausted), None, None, None);
        }
    }
    match crate::checkout::dispatch(method, params, state, outbound.clone()).await {
        Ok(dispatched) => (Ok(dispatched.value), None, dispatched.subscription, None),
        Err(error) => (Err(error), None, None, None),
    }
}

async fn receive(stream: &mut SplitStream<WebSocket>) -> Option<Result<ClientMessage, ErrorCode>> {
    loop {
        match stream.next().await? {
            Ok(Message::Text(text)) => {
                return Some(serde_json::from_str(&text).map_err(|_| ErrorCode::InvalidMessage));
            }
            Ok(Message::Ping(_) | Message::Pong(_)) => {}
            Ok(Message::Close(_)) | Err(_) => return None,
            Ok(Message::Binary(_)) => return Some(Err(ErrorCode::InvalidMessage)),
        }
    }
}

fn error(
    outbound: &Outbound,
    request_id: Option<String>,
    code: ErrorCode,
) -> Result<(), QueueError> {
    outbound.send(&ServerMessage::Error {
        request_id,
        code,
        message: code.message().to_owned(),
        retryable: code.retryable(),
    })
}

fn dispatch(
    method: &str,
    params: &Value,
    state: &Shared,
    capabilities: &[String],
    subscriptions: &mut ConnectionSubscriptions,
) -> Result<Value, ErrorCode> {
    let capability = match method {
        "server.info"
        | "connection.ping"
        | "server.status.subscribe"
        | "subscription.release.request" => method,
        "server.status.unsubscribe" => "server.status.subscribe",
        _ => return Err(ErrorCode::MethodNotFound),
    };
    if !capabilities.iter().any(|c| c == capability) {
        return Err(ErrorCode::UnsupportedCapability);
    }
    match method {
        "server.info" => serde_json::to_value(state.info()).map_err(|_| ErrorCode::InvalidMessage),
        "connection.ping" => {
            let nonce = params
                .get("nonce")
                .and_then(Value::as_str)
                .filter(|s| valid_id(s))
                .ok_or(ErrorCode::InvalidMessage)?;
            Ok(json!({"nonce":nonce}))
        }
        "server.status.subscribe" => {
            if subscriptions.len() >= MAX_SUBSCRIPTIONS {
                return Err(ErrorCode::ResourceExhausted);
            }
            let id = Uuid::new_v4().to_string();
            subscriptions.status.insert(id.clone());
            Ok(json!({"subscription_id":id}))
        }
        "server.status.unsubscribe" => {
            let id = params
                .get("subscription_id")
                .and_then(Value::as_str)
                .ok_or(ErrorCode::InvalidMessage)?;
            if !subscriptions.status.remove(id) {
                return Err(ErrorCode::SubscriptionNotFound);
            }
            Ok(json!({"unsubscribed":true}))
        }
        "subscription.release.request" => {
            let request: SubscriptionReleaseRequest =
                serde_json::from_value(params.clone()).map_err(|_| ErrorCode::InvalidMessage)?;
            if !valid_id(&request.subscription_id) {
                return Err(ErrorCode::InvalidMessage);
            }
            subscriptions.status.remove(&request.subscription_id);
            subscriptions.labels.remove(&request.subscription_id);
            subscriptions.diffs.remove(&request.subscription_id);
            serde_json::to_value(SubscriptionReleaseResult {
                subscription_id: request.subscription_id,
            })
            .map_err(|_| ErrorCode::InvalidMessage)
        }
        _ => Err(ErrorCode::MethodNotFound),
    }
}
