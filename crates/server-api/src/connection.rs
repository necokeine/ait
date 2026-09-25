use std::ops::ControlFlow;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::ws::{CloseFrame, Message, WebSocket, close_code};
use futures_util::{SinkExt, StreamExt, stream::SplitStream};
use server_protocol::methods::InboundKind;
use server_protocol::{ClientMessage, ErrorCode, ServerMessage, valid_id};
use tokio::time::timeout;

use crate::Shared;
use crate::outbound::{Frame, Outbound, QueueError};
use server_model::{Context, Request};

mod dispatch;
mod routing;

const HELLO_TIMEOUT: Duration = Duration::from_secs(10);
const WRITE_TIMEOUT: Duration = Duration::from_secs(5);
const CLOSE_TIMEOUT: Duration = Duration::from_secs(1);
pub(super) const MAX_SUBSCRIPTIONS: usize = 16;

#[derive(Default)]
pub(super) struct ConnectionSubscriptions {
    metadata: server_metadata::connection::Connection,
    filesystem: server_filesystem::connection::Connection,
    terminals: server_terminal::connection::TerminalConnection,
}

impl ConnectionSubscriptions {
    fn len(&self) -> usize {
        self.metadata
            .len()
            .saturating_add(self.filesystem.len())
            .saturating_add(self.terminals.len())
    }

    fn release(&mut self, id: &str) {
        self.metadata.release(id);
        self.filesystem.release(id);
        self.terminals.release(id);
    }
}

fn websocket_frame(frame: Frame) -> Message {
    match frame {
        Frame::Text(text) => Message::Text(text.into()),
        Frame::Binary(bytes) => Message::Binary(bytes.into()),
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
                timeout(WRITE_TIMEOUT, sink.send(websocket_frame(queued.message))).await,
                Ok(Ok(()))
            ) {
                break;
            }
        }
        stopped.cancel();
        let _ = timeout(CLOSE_TIMEOUT, async {
            // Flush already bounded diagnostic/error/status messages before closing.
            while let Ok(queued) = receiver.try_recv() {
                sink.send(websocket_frame(queued.message)).await?;
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
    let session = state.metadata.session_events.connect();
    outbound.send(&ServerMessage::ServerInfo {
        info: state.info(),
        connection_id: session.id().to_owned(),
        negotiated_capabilities: capabilities.clone(),
    })?;
    let mut subscriptions = ConnectionSubscriptions {
        metadata: server_metadata::connection::Connection::new(session),
        ..Default::default()
    };
    let mut terminal_poll = tokio::time::interval(Duration::from_millis(40));
    terminal_poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut upload_expiry = tokio::time::interval(Duration::from_secs(30));
    loop {
        subscriptions.filesystem.files.prune_uploads();
        let message = tokio::select! {
            biased;
            () = state.cancellation.cancelled() => {
                subscriptions.metadata.draining(outbound)?;
                return Ok(());
            },
            _ = terminal_poll.tick(), if !subscriptions.terminals.is_empty() => {
                subscriptions.terminals.poll(&state.terminal, outbound).await?;
                continue;
            },
            message = receive(&mut stream) => message,
            _ = upload_expiry.tick() => {
                subscriptions.filesystem.files.prune_uploads();
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
                if let Err(code) =
                    server_metadata::connection::session::heartbeat(params, &subscriptions.metadata)
                {
                    error(outbound, None, code)?;
                }
                return Ok(ControlFlow::Continue(()));
            }
            if method == "terminal.input"
                && state.terminal.terminals.is_some()
                && capabilities.iter().any(|capability| capability == &method)
            {
                if let Err(code) = subscriptions.terminals.event(params, &state.terminal).await {
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
            } else if state.terminal.terminals.is_none() {
                Err(ErrorCode::NotImplemented)
            } else {
                subscriptions
                    .terminals
                    .binary(&bytes, &state.terminal)
                    .await
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
                .filesystem
                .files
                .frame(id, frame, &state.filesystem, outbound)
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

async fn process_request(
    request: Request,
    state: &Shared,
    outbound: &Outbound,
    capabilities: &[String],
    subscriptions: &mut ConnectionSubscriptions,
) -> Result<(), QueueError> {
    let handler = match request_handler(&request.method, state, capabilities) {
        Ok(handler) => handler,
        Err(code) => return error(outbound, Some(request.id), code),
    };
    dispatch::request(
        handler,
        Context {
            request,
            runtime: state,
            outbound,
            available_subscriptions: MAX_SUBSCRIPTIONS.saturating_sub(subscriptions.len()),
        },
        state,
        subscriptions,
    )
    .await
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
