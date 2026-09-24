use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::Duration;

use axum::extract::ws::{CloseFrame, Message, WebSocket, close_code};
use futures_util::{SinkExt, StreamExt, stream::SplitStream};
use serde_json::Value;
use server_metadata::service::workspace_labels::WorkspaceLabelSubscription;
use server_protocol::methods::InboundKind;
use server_protocol::{ClientMessage, ErrorCode, Lifecycle, ServerMessage, valid_id};
use tokio::time::timeout;
use uuid::Uuid;

use crate::Shared;
use crate::checkout::CheckoutDiffSubscription;
use crate::outbound::{Outbound, QueueError};

mod routing;

const HELLO_TIMEOUT: Duration = Duration::from_secs(10);
const WRITE_TIMEOUT: Duration = Duration::from_secs(5);
const CLOSE_TIMEOUT: Duration = Duration::from_secs(1);
const MAX_SUBSCRIPTIONS: usize = 16;

#[derive(Default)]
struct ConnectionSubscriptions {
    files: crate::files::FileConnection,
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
            .saturating_add(self.files.len())
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
                timeout(WRITE_TIMEOUT, sink.send(queued.message)).await,
                Ok(Ok(()))
            ) {
                break;
            }
        }
        stopped.cancel();
        let _ = timeout(CLOSE_TIMEOUT, async {
            // Flush already bounded diagnostic/error/status messages before closing.
            while let Ok(queued) = receiver.try_recv() {
                sink.send(queued.message).await?;
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
    // Release connection-owned observers, including pending Agent waits, on clean disconnect.
    outbound_failure.cancel();
}

async fn read(
    mut stream: SplitStream<WebSocket>,
    state: &Shared,
    outbound: &Outbound,
) -> Result<(), QueueError> {
    let hello = tokio::select! {
        () = state.cancellation.cancelled() => return Ok(()),
        result = timeout(HELLO_TIMEOUT, receive(&mut stream)) => match result {
            Ok(Some(Ok(Incoming::Text(ClientMessage::Hello(hello))))) => hello,
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
    let mut upload_expiry = tokio::time::interval(Duration::from_secs(30));
    loop {
        subscriptions.files.prune_uploads();
        let message = tokio::select! {
            biased;
            () = state.cancellation.cancelled() => {
                for subscription_id in subscriptions.status {
                    outbound.send(&ServerMessage::Status {
                        subscription_id,
                        lifecycle: Lifecycle::Draining,
                    })?;
                }
                return Ok(());
            },
            message = receive(&mut stream) => message,
            _ = upload_expiry.tick() => {
                subscriptions.files.prune_uploads();
                continue;
            },
        };
        match message {
            Some(Ok(Incoming::Text(ClientMessage::Request {
                request_id,
                method,
                params,
            }))) if valid_id(&request_id) && valid_id(&method) => {
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
            Some(Ok(Incoming::Text(ClientMessage::Event { method, .. }))) if valid_id(&method) => {
                let code = routing::placeholder(&method, InboundKind::Event, &capabilities);
                error(outbound, None, code)?;
            }
            Some(Ok(Incoming::Text(ClientMessage::Response {
                request_id, method, ..
            }))) if valid_id(&method) && request_id.as_deref().is_none_or(valid_id) => {
                let code = routing::placeholder(&method, InboundKind::Response, &capabilities);
                error(outbound, request_id, code)?;
            }
            None => return Ok(()),
            Some(Ok(Incoming::Binary(bytes)))
                if capabilities
                    .iter()
                    .any(|method| method == "file.upload.request") =>
            {
                let Some((id, frame)) = server_filesystem::protocol::file_transfer::decode(&bytes)
                else {
                    return error(outbound, None, ErrorCode::InvalidMessage);
                };
                subscriptions
                    .files
                    .frame(id, frame, state, outbound)
                    .await?;
            }
            Some(
                Ok(
                    Incoming::Text(
                        ClientMessage::Hello(_)
                        | ClientMessage::Request { .. }
                        | ClientMessage::Event { .. }
                        | ClientMessage::Response { .. },
                    )
                    | Incoming::Binary(_),
                )
                | Err(_),
            ) => {
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
    let Some(route) = routing::lookup(&method) else {
        return error(outbound, Some(request_id), ErrorCode::MethodNotFound);
    };
    if route.kind != InboundKind::Request {
        return error(outbound, Some(request_id), ErrorCode::InvalidMessage);
    }
    if !capabilities
        .iter()
        .any(|negotiated| negotiated == route.capability)
    {
        return error(outbound, Some(request_id), ErrorCode::UnsupportedCapability);
    }
    if method != "server.status.unsubscribe"
        && !state
            .info
            .implemented_capabilities
            .iter()
            .any(|implemented| implemented == &method)
    {
        return error(outbound, Some(request_id), ErrorCode::NotImplemented);
    }
    let Some(handler) = route.handler else {
        return error(outbound, Some(request_id), ErrorCode::NotImplemented);
    };
    if method == "agent.finish.wait.request" {
        return crate::agent_execution::wait(request_id, params, state, outbound);
    }
    if handler == routing::Handler::Files {
        let available_subscriptions = MAX_SUBSCRIPTIONS.saturating_sub(subscriptions.len());
        return subscriptions
            .files
            .request(
                crate::files::FileRequest {
                    id: request_id,
                    method,
                    params,
                    available_subscriptions,
                },
                state,
                outbound,
            )
            .await;
    }
    let routing::RouteResult {
        result,
        label_subscription,
        diff_subscription,
        workspace_event,
    } = routing::route_request(handler, &method, params, state, outbound, subscriptions).await;
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
    if let Some(pending) = label_subscription {
        let (subscription_id, subscription) = pending.activate()?;
        subscriptions.labels.insert(subscription_id, subscription);
    }
    if let Some(pending) = diff_subscription {
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

enum Incoming {
    Text(ClientMessage),
    Binary(Vec<u8>),
}

async fn receive(stream: &mut SplitStream<WebSocket>) -> Option<Result<Incoming, ErrorCode>> {
    loop {
        match stream.next().await? {
            Ok(Message::Text(text)) => {
                return Some(
                    serde_json::from_str(&text)
                        .map(Incoming::Text)
                        .map_err(|_| ErrorCode::InvalidMessage),
                );
            }
            Ok(Message::Ping(_) | Message::Pong(_)) => {}
            Ok(Message::Close(_)) | Err(_) => return None,
            Ok(Message::Binary(bytes)) => return Some(Ok(Incoming::Binary(bytes.to_vec()))),
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
