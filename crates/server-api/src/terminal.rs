use std::collections::BTreeMap;

use serde_json::{Value, json};
use server_protocol::{ErrorCode, ServerMessage};
use server_terminal::ports::Observation;
use server_terminal::protocol::{self as wire, Input, Opcode};
use server_terminal::rpc::decode;
use server_terminal::service::Terminals;
use uuid::Uuid;

use crate::Shared;
use crate::outbound::{Outbound, QueueError};

#[derive(Default)]
pub(crate) struct TerminalConnection {
    owner: String,
    streams: BTreeMap<String, Stream>,
    lists: BTreeMap<String, Listing>,
    next_slot: u8,
}

#[derive(Clone)]
struct Stream {
    terminal: String,
    slot: u8,
    revision: u64,
    size: wire::Size,
    restore: Option<wire::Restore>,
}

#[derive(Clone)]
struct Listing {
    filter: wire::ListRequest,
    previous: Vec<wire::TerminalInfo>,
}

pub(crate) struct Request {
    pub id: String,
    pub method: String,
    pub params: Value,
    pub available: usize,
}

impl TerminalConnection {
    pub(crate) fn len(&self) -> usize {
        self.streams.len() + self.lists.len()
    }

    pub(crate) fn release(&mut self, id: &str) {
        self.streams.remove(id);
        self.lists.remove(id);
    }

    fn owner(&mut self) -> String {
        if self.owner.is_empty() {
            self.owner = Uuid::new_v4().to_string();
        }
        self.owner.clone()
    }

    pub(crate) async fn request(
        &mut self,
        request: Request,
        state: &Shared,
        outbound: &Outbound,
    ) -> Result<(), QueueError> {
        let Request {
            id,
            method,
            params,
            available,
        } = request;
        let result = match method.as_str() {
            "terminal.subscribe.request" => {
                return self
                    .subscribe((id, params, available), state, outbound)
                    .await;
            }
            "terminal.list.subscribe.request" => {
                return self
                    .subscribe_list((id, params, available), state, outbound)
                    .await;
            }
            "terminal.unsubscribe.request" => decode::<wire::TerminalRequest>(params)
                .map(|request| {
                    self.streams
                        .retain(|_, stream| stream.terminal != request.terminal_id);
                    json!({"terminalId":request.terminal_id})
                })
                .map_err(Into::into),
            "terminal.list.unsubscribe.request" => decode::<wire::ListRequest>(params)
                .and_then(|request| {
                    if request.cwd.is_none() {
                        return Err(server_terminal::Error::Invalid);
                    }
                    self.lists.retain(|_, listing| listing.filter != request);
                    Ok(json!({"cwd":request.cwd,"workspaceId":request.workspace_id}))
                })
                .map_err(Into::into),
            _ => {
                run(state, move |service| {
                    server_terminal::rpc::execute(service, &method, params)
                })
                .await
            }
        };
        respond(outbound, id, result)
    }

    async fn subscribe(
        &mut self,
        input: (String, Value, usize),
        state: &Shared,
        outbound: &Outbound,
    ) -> Result<(), QueueError> {
        let (id, params, available) = input;
        let request = match decode::<wire::SubscribeRequest>(params) {
            Ok(request) => request,
            Err(error) => return respond(outbound, id, Err(error.into())),
        };
        let replaced = self
            .streams
            .values()
            .any(|stream| stream.terminal == request.terminal_id);
        if available == 0 && !replaced {
            return respond(outbound, id, Err(ErrorCode::ResourceExhausted));
        }
        let slot = (0..=u8::MAX)
            .map(|offset| self.next_slot.wrapping_add(offset))
            .find(|slot| self.streams.values().all(|stream| stream.slot != *slot));
        let Some(slot) = slot else {
            return respond(outbound, id, Err(ErrorCode::ResourceExhausted));
        };
        let terminal = request.terminal_id.clone();
        let restore = request.restore.clone();
        let owner = self.owner();
        let initial = run(state, move |service| {
            if let Some(size) = restore.as_ref().and_then(|restore| restore.size) {
                service.input(
                    &terminal,
                    &owner,
                    &Input::Resize(wire::Resize {
                        size,
                        intent: wire::ResizeIntent::Claim,
                    }),
                )?;
            }
            service.observe(&terminal, None, restore.as_ref())
        })
        .await;
        let initial = match initial {
            Ok(initial) => initial,
            Err(code) => {
                return respond(
                    outbound,
                    id,
                    Ok(json!({"terminalId":request.terminal_id,"error":code.message()})),
                );
            }
        };
        self.streams
            .retain(|_, stream| stream.terminal != request.terminal_id);
        let subscription = Uuid::new_v4().to_string();
        respond(
            outbound,
            id,
            Ok(
                json!({"terminalId":request.terminal_id,"subscriptionId":subscription,"slot":slot,"error":null}),
            ),
        )?;
        self.next_slot = slot.wrapping_add(1);
        let stream = Stream {
            terminal: request.terminal_id,
            slot,
            revision: initial.revision,
            size: initial.size,
            restore: request.restore,
        };
        let exited = initial.exited;
        deliver(outbound, &subscription, &stream, initial).await?;
        if !exited {
            self.streams.insert(subscription, stream);
        }
        Ok(())
    }

    async fn subscribe_list(
        &mut self,
        input: (String, Value, usize),
        state: &Shared,
        outbound: &Outbound,
    ) -> Result<(), QueueError> {
        let (id, params, available) = input;
        let filter = match decode::<wire::ListRequest>(params) {
            Ok(filter) if filter.cwd.is_some() => filter,
            _ => return respond(outbound, id, Err(ErrorCode::InvalidMessage)),
        };
        if available == 0 && !self.lists.values().any(|listing| listing.filter == filter) {
            return respond(outbound, id, Err(ErrorCode::ResourceExhausted));
        }
        let requested = filter.clone();
        let initial = match run(state, move |service| service.list(&requested)).await {
            Ok(initial) => initial,
            Err(error) => return respond(outbound, id, Err(error)),
        };
        self.lists.retain(|_, listing| listing.filter != filter);
        let subscription = Uuid::new_v4().to_string();
        respond(
            outbound,
            id,
            Ok(listing_value(&subscription, &filter, &initial)),
        )?;
        self.lists.insert(
            subscription,
            Listing {
                filter,
                previous: initial,
            },
        );
        Ok(())
    }

    pub(crate) async fn event(&mut self, params: Value, state: &Shared) -> Result<(), ErrorCode> {
        let request: wire::InputRequest = decode(params)?;
        self.input(request.terminal_id, request.message, state)
            .await
    }

    pub(crate) async fn binary(&mut self, bytes: &[u8], state: &Shared) -> Result<(), ErrorCode> {
        let (slot, input) = wire::client_frame(bytes)?;
        let terminal = self
            .streams
            .values()
            .find(|stream| stream.slot == slot)
            .ok_or(ErrorCode::SubscriptionNotFound)?
            .terminal
            .clone();
        self.input(terminal, input, state).await
    }

    async fn input(
        &mut self,
        terminal: String,
        input: Input,
        state: &Shared,
    ) -> Result<(), ErrorCode> {
        let owner = self.owner();
        run(state, move |service| {
            service.input(&terminal, &owner, &input)
        })
        .await
    }

    pub(crate) async fn poll(
        &mut self,
        state: &Shared,
        outbound: &Outbound,
    ) -> Result<(), QueueError> {
        if self.len() == 0 {
            return Ok(());
        }
        let streams = self.streams.clone();
        let lists = self.lists.clone();
        let observed = run(state, move |service| {
            let streams: Vec<_> = streams
                .into_iter()
                .map(|(id, stream)| {
                    let result = service.observe(
                        &stream.terminal,
                        Some(stream.revision),
                        stream.restore.as_ref(),
                    );
                    (id, result)
                })
                .collect();
            let lists: Vec<_> = lists
                .into_iter()
                .map(|(id, listing)| (id, service.list(&listing.filter)))
                .collect();
            Ok((streams, lists))
        })
        .await;
        let (streams, lists) = match observed {
            Ok(observed) => observed,
            Err(ErrorCode::ResourceExhausted | ErrorCode::ServerDraining) => return Ok(()),
            Err(code) => return send_error(outbound, code),
        };
        for (id, observed) in streams {
            let Some(stream) = self.streams.get_mut(&id) else {
                continue;
            };
            match observed {
                Ok(observed) => {
                    let exited = observed.exited;
                    let revision = observed.revision;
                    let size = observed.size;
                    deliver(outbound, &id, stream, observed).await?;
                    stream.revision = revision;
                    stream.size = size;
                    if exited {
                        self.streams.remove(&id);
                    }
                }
                Err(error) => {
                    let mut params = json!({"subscriptionId":id,"terminalId":stream.terminal});
                    if error != server_terminal::Error::NotFound {
                        params["error"] = json!(error.to_string());
                    }
                    outbound.send(&ServerMessage::Event {
                        method: "terminal.stream.exit".to_owned(),
                        params,
                    })?;
                    self.streams.remove(&id);
                }
            }
        }
        for (id, result) in lists {
            let Some(listing) = self.lists.get_mut(&id) else {
                continue;
            };
            let Ok(next) = result else {
                continue;
            };
            if next != listing.previous {
                outbound.send(&ServerMessage::Event {
                    method: "terminal.list.changed".to_owned(),
                    params: listing_value(&id, &listing.filter, &next),
                })?;
                listing.previous = next;
            }
        }
        Ok(())
    }
}

fn listing_value(id: &str, filter: &wire::ListRequest, terminals: &[wire::TerminalInfo]) -> Value {
    let mut value = json!({"subscriptionId":id,"cwd":filter.cwd,"terminals":terminals});
    if let Some(workspace_id) = &filter.workspace_id {
        value["workspaceId"] = json!(workspace_id);
    }
    value
}

async fn deliver(
    outbound: &Outbound,
    id: &str,
    stream: &Stream,
    observed: Observation,
) -> Result<(), QueueError> {
    if observed.size != stream.size {
        let resize = serde_json::to_vec(&observed.size)?;
        outbound
            .binary(wire::frame(Opcode::Resize, stream.slot, &resize))
            .await?;
    }
    for (opcode, bytes) in observed.frames {
        outbound
            .binary(wire::frame(opcode, stream.slot, &bytes))
            .await?;
    }
    if observed.exited {
        outbound.send(&ServerMessage::Event {
            method: "terminal.stream.exit".to_owned(),
            params: json!({"subscriptionId":id,"terminalId":stream.terminal}),
        })?;
    }
    Ok(())
}

pub(super) async fn run<R: Send + 'static>(
    state: &Shared,
    execute: impl FnOnce(&mut Terminals) -> Result<R, server_terminal::Error> + Send + 'static,
) -> Result<R, ErrorCode> {
    let service = state.terminals.clone().ok_or(ErrorCode::NotImplemented)?;
    let admission = state.admission.lock().map_err(|_| ErrorCode::TerminalIo)?;
    if state.cancellation.is_cancelled() {
        return Err(ErrorCode::ServerDraining);
    }
    let permit = state
        .terminal_jobs
        .clone()
        .try_acquire_owned()
        .map_err(|_| ErrorCode::ResourceExhausted)?;
    let tracking = state.tasks.token();
    let job = tokio::task::spawn_blocking(move || {
        let (_permit, _tracking) = (permit, tracking);
        execute(&mut *service.lock().map_err(|_| server_terminal::Error::Io)?)
    });
    drop(admission);
    job.await
        .map_err(|_| ErrorCode::TerminalIo)?
        .map_err(Into::into)
}

fn respond(
    outbound: &Outbound,
    id: String,
    result: Result<Value, ErrorCode>,
) -> Result<(), QueueError> {
    match result {
        Ok(result) => outbound.send(&ServerMessage::Response {
            request_id: id,
            result,
        }),
        Err(code) => outbound.send(&ServerMessage::Error {
            request_id: Some(id),
            code,
            message: code.message().to_owned(),
            retryable: code.retryable(),
        }),
    }
}

pub(super) fn send_error(outbound: &Outbound, code: ErrorCode) -> Result<(), QueueError> {
    outbound.send(&ServerMessage::Error {
        request_id: None,
        code,
        message: code.message().to_owned(),
        retryable: code.retryable(),
    })
}

/// Reconcile archive/removal even when no WebSocket clients remain connected.
pub(super) fn maintain(state: &std::sync::Arc<Shared>) {
    if state.terminals.is_none() {
        return;
    }
    let weak = std::sync::Arc::downgrade(state);
    let cancellation = state.cancellation.clone();
    state.tasks.spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_millis(250));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! { biased;
                () = cancellation.cancelled() => break,
                _ = interval.tick() => {},
            }
            let Some(state) = weak.upgrade() else {
                break;
            };
            if let Err(error) = run(&state, Terminals::reconcile).await
                && !matches!(
                    error,
                    ErrorCode::ServerDraining | ErrorCode::ResourceExhausted
                )
            {
                tracing::error!("terminal workspace reconciliation failed");
            }
        }
    });
}
