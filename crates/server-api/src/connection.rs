use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::ws::{CloseFrame, Message, WebSocket, close_code};
use futures_util::{SinkExt, StreamExt, stream::SplitStream};
use serde_json::{Value, json};
use server_protocol::{ClientMessage, ErrorCode, Lifecycle, ServerMessage, valid_id};
use tokio::time::timeout;
use uuid::Uuid;

use crate::Shared;
use crate::outbound::{Outbound, QueueError};

const HELLO_TIMEOUT: Duration = Duration::from_secs(10);
const WRITE_TIMEOUT: Duration = Duration::from_secs(5);
const CLOSE_TIMEOUT: Duration = Duration::from_secs(1);
const MAX_SUBSCRIPTIONS: usize = 16;

pub(super) async fn serve(socket: WebSocket, state: Arc<Shared>) {
    let (mut sink, stream) = socket.split();
    let (outbound, mut receiver) = Outbound::new();
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
    let capabilities = match hello.negotiate() {
        Ok(capabilities) => capabilities,
        Err(code) => return error(outbound, None, code),
    };
    outbound.send(&ServerMessage::ServerInfo {
        info: state.info(),
        connection_id: Uuid::new_v4().to_string(),
        negotiated_capabilities: capabilities.clone(),
    })?;
    let mut subscriptions = BTreeSet::new();
    loop {
        let message = tokio::select! {
            biased;
            () = state.cancellation.cancelled() => {
                for subscription_id in subscriptions {
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
                let result = dispatch(&method, &params, state, &capabilities, &mut subscriptions);
                match result {
                    Ok(value) => {
                        let subscription_id = (method == "server.status.subscribe")
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
                        if let Some(subscription_id) = subscription_id {
                            outbound.send(&ServerMessage::Status {
                                subscription_id,
                                lifecycle: state.info().lifecycle,
                            })?;
                        }
                    }
                    Err(code) => error(outbound, Some(request_id), code)?,
                }
            }
            None => return Ok(()),
            Some(Ok(ClientMessage::Hello(_) | ClientMessage::Request { .. }) | Err(_)) => {
                return error(outbound, None, ErrorCode::InvalidMessage);
            }
        }
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
    outbound.send(&ServerMessage::Error { request_id, code })
}

fn dispatch(
    method: &str,
    params: &Value,
    state: &Shared,
    capabilities: &[String],
    subscriptions: &mut BTreeSet<String>,
) -> Result<Value, ErrorCode> {
    let capability = match method {
        "server.info" | "connection.ping" | "server.status.subscribe" => method,
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
            subscriptions.insert(id.clone());
            Ok(json!({"subscription_id":id}))
        }
        "server.status.unsubscribe" => {
            let id = params
                .get("subscription_id")
                .and_then(Value::as_str)
                .ok_or(ErrorCode::InvalidMessage)?;
            if !subscriptions.remove(id) {
                return Err(ErrorCode::SubscriptionNotFound);
            }
            Ok(json!({"unsubscribed":true}))
        }
        _ => Err(ErrorCode::MethodNotFound),
    }
}
