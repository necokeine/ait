use std::collections::{BTreeMap, BTreeSet};
use std::ops::ControlFlow;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::ws::{CloseFrame, Message, WebSocket, close_code};
use futures_util::{SinkExt, StreamExt, stream::SplitStream};
use serde_json::Value;
use server_metadata::service::session::{SessionConnection, SessionSubscription};
use server_metadata::service::workspace_labels::WorkspaceLabelSubscription;
use server_protocol::methods::InboundKind;
use server_protocol::{ClientMessage, ErrorCode, Lifecycle, ServerMessage, valid_id};
use tokio::time::timeout;

use crate::Shared;
use crate::checkout::CheckoutDiffSubscription;
use crate::outbound::{Outbound, QueueError};

mod routing;

const HELLO_TIMEOUT: Duration = Duration::from_secs(10);
const WRITE_TIMEOUT: Duration = Duration::from_secs(5);
const CLOSE_TIMEOUT: Duration = Duration::from_secs(1);
pub(super) const MAX_SUBSCRIPTIONS: usize = 16;

#[derive(Default)]
pub(super) struct ConnectionSubscriptions {
    files: crate::files::FileConnection,
    terminals: crate::terminal::TerminalConnection,
    diffs: BTreeMap<String, CheckoutDiffSubscription>,
    status: BTreeSet<String>,
    labels: BTreeMap<String, WorkspaceLabelSubscription>,
    pub(super) events: BTreeMap<String, SessionSubscription>,
    pub(super) session: Option<SessionConnection>,
}

impl ConnectionSubscriptions {
    pub(super) fn len(&self) -> usize {
        self.status
            .len()
            .saturating_add(self.labels.len())
            .saturating_add(self.diffs.len())
            .saturating_add(self.files.len())
            .saturating_add(self.terminals.len())
            .saturating_add(self.events.len())
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
    let session = state.session_events.connect();
    outbound.send(&ServerMessage::ServerInfo {
        info: state.info(),
        connection_id: session.id().to_owned(),
        negotiated_capabilities: capabilities.clone(),
    })?;
    let mut subscriptions = ConnectionSubscriptions {
        session: Some(session),
        ..Default::default()
    };
    let mut terminal_poll = tokio::time::interval(Duration::from_millis(40));
    terminal_poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
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
            _ = terminal_poll.tick(), if subscriptions.terminals.len() != 0 => {
                subscriptions.terminals.poll(state, outbound).await?;
                continue;
            },
            message = receive(&mut stream) => message,
            _ = upload_expiry.tick() => {
                subscriptions.files.prune_uploads();
                continue;
            },
        };
        if process_message(message, state, outbound, &capabilities, &mut subscriptions)
            .await?
            .is_break()
        {
            return Ok(());
        }
    }
}

async fn process_message(
    message: Option<Result<Incoming, ErrorCode>>,
    state: &Shared,
    outbound: &Outbound,
    capabilities: &[String],
    subscriptions: &mut ConnectionSubscriptions,
) -> Result<ControlFlow<()>, QueueError> {
    match message {
        Some(Ok(Incoming::Text(ClientMessage::Request {
            request_id,
            method,
            params,
        }))) if valid_id(&request_id) && valid_id(&method) => {
            process_request(
                Request {
                    id: request_id,
                    method,
                    params,
                },
                state,
                outbound,
                capabilities,
                subscriptions,
            )
            .await?;
        }
        Some(Ok(Incoming::Text(ClientMessage::Event { method, params }))) if valid_id(&method) => {
            if method == server_metadata::protocol::server::HEARTBEAT_METHOD
                && capabilities.iter().any(|capability| capability == &method)
            {
                if let Err(code) = crate::session::heartbeat(params, subscriptions) {
                    error(outbound, None, code)?;
                }
                return Ok(ControlFlow::Continue(()));
            }
            if method == "terminal.input"
                && state.terminals.is_some()
                && capabilities.iter().any(|capability| capability == &method)
            {
                if let Err(code) = subscriptions.terminals.event(params, state).await {
                    error(outbound, None, code)?;
                }
                return Ok(ControlFlow::Continue(()));
            }
            let code = routing::placeholder(&method, InboundKind::Event, capabilities);
            error(outbound, None, code)?;
        }
        Some(Ok(Incoming::Text(ClientMessage::Response {
            request_id, method, ..
        }))) if valid_id(&method) && request_id.as_deref().is_none_or(valid_id) => {
            let code = routing::placeholder(&method, InboundKind::Response, capabilities);
            error(outbound, request_id, code)?;
        }
        None => return Ok(ControlFlow::Break(())),
        Some(Ok(Incoming::Binary(bytes))) if bytes.first().is_some_and(|opcode| *opcode < 0x10) => {
            let code = if !capabilities.iter().any(|method| method == "terminal.input") {
                Err(ErrorCode::UnsupportedCapability)
            } else if state.terminals.is_none() {
                Err(ErrorCode::NotImplemented)
            } else {
                subscriptions.terminals.binary(&bytes, state).await
            };
            if let Err(code) = code {
                error(outbound, None, code)?;
            }
        }
        Some(Ok(Incoming::Binary(bytes)))
            if capabilities
                .iter()
                .any(|method| method == "file.upload.request") =>
        {
            let Some((id, frame)) = server_filesystem::protocol::file_transfer::decode(&bytes)
            else {
                error(outbound, None, ErrorCode::InvalidMessage)?;
                return Ok(ControlFlow::Break(()));
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
            error(outbound, None, ErrorCode::InvalidMessage)?;
            return Ok(ControlFlow::Break(()));
        }
    }
    Ok(ControlFlow::Continue(()))
}

struct Request {
    id: String,
    method: String,
    params: Value,
}

async fn process_request(
    request: Request,
    state: &Shared,
    outbound: &Outbound,
    capabilities: &[String],
    subscriptions: &mut ConnectionSubscriptions,
) -> Result<(), QueueError> {
    let Request {
        id: request_id,
        method,
        params,
    } = request;
    let handler = match request_handler(&method, state, capabilities) {
        Ok(handler) => handler,
        Err(code) => return error(outbound, Some(request_id), code),
    };
    if method == "agent.finish.wait.request" {
        return crate::agent_execution::wait(request_id, params, state, outbound);
    }
    if handler == routing::Handler::Session {
        return crate::session::subscribe(request_id, params, state, outbound, subscriptions);
    }
    if handler == routing::Handler::Terminal {
        let available = MAX_SUBSCRIPTIONS.saturating_sub(subscriptions.len());
        return subscriptions
            .terminals
            .request(
                crate::terminal::Request {
                    id: request_id,
                    method,
                    params,
                    available,
                },
                state,
                outbound,
            )
            .await;
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

fn request_handler(
    method: &str,
    state: &Shared,
    capabilities: &[String],
) -> Result<routing::Handler, ErrorCode> {
    let route = routing::lookup(method).ok_or(ErrorCode::MethodNotFound)?;
    if route.kind != InboundKind::Request {
        return Err(ErrorCode::InvalidMessage);
    }
    if !capabilities
        .iter()
        .any(|negotiated| negotiated == route.capability)
    {
        return Err(ErrorCode::UnsupportedCapability);
    }
    if method != "server.status.unsubscribe"
        && !state
            .info
            .implemented_capabilities
            .iter()
            .any(|implemented| implemented == method)
    {
        return Err(ErrorCode::NotImplemented);
    }
    route.handler.ok_or(ErrorCode::NotImplemented)
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
