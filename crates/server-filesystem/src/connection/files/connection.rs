use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::protocol::{file_transfer::FileFrame, files as wire};
use crate::service::files::Files;
use crate::service::uploads::{UploadStep, Uploads};
use serde_json::{Value, json};
use server_model::{ErrorCode, ServerMessage, valid_id};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::dispatch::State as Shared;
use server_model::outbound::{Outbound, QueueError};

#[derive(Default)]
/// File subscriptions and unfinished uploads owned by one physical connection.
pub struct FileConnection {
    subscriptions: BTreeMap<String, Subscription>,
    uploads: Uploads,
}

struct Subscription {
    active: Arc<Mutex<bool>>,
    cancel: CancellationToken,
}

impl Drop for Subscription {
    fn drop(&mut self) {
        *self
            .active
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = false;
        self.cancel.cancel();
    }
}

impl FileConnection {
    /// Count active file observers.
    #[must_use]
    pub fn len(&self) -> usize {
        self.subscriptions.len()
    }

    /// Whether no file observers remain.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Release a matching observer and prevent subsequent deliveries.
    pub fn release(&mut self, id: &str) {
        self.subscriptions.remove(id);
    }

    /// Drop expired uploads and their unfinished temporary files.
    pub fn prune_uploads(&mut self) {
        self.uploads.prune();
    }

    pub(crate) async fn request(
        &mut self,
        request: FileRequest,
        state: &Shared,
        outbound: &Outbound,
    ) -> Result<(), QueueError> {
        let FileRequest {
            id,
            method,
            params,
            available_subscriptions,
        } = request;
        let result = match method.as_str() {
            "fs.file.subscribe.request" => {
                return self
                    .subscribe((id, params, available_subscriptions), state, outbound)
                    .await;
            }
            "fs.file.unsubscribe.request" => super::decode::<wire::UnsubscribeRequest>(params)
                .and_then(|request| {
                    if !valid_id(&request.subscription_id) {
                        return Err(ErrorCode::InvalidMessage);
                    }
                    self.release(&request.subscription_id);
                    Ok(json!({"subscriptionId":request.subscription_id}))
                }),
            "file.upload.request" => match self.uploads.begin(&id, params).map_err(Into::into) {
                Ok(()) => return Ok(()),
                Err(error) => Err(error),
            },
            "fs.explorer.request"
                if params.get("acceptBinary") == Some(&Value::Bool(true))
                    && params.get("mode").and_then(Value::as_str) == Some("file") =>
            {
                return super::transfer::stream_preview(id, params, state, outbound).await;
            }
            _ => super::execute(method, params, state).await,
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
        let request = match super::decode::<wire::SubscribeRequest>(params) {
            Ok(request) => request,
            Err(error) => return respond(outbound, id, Err(error)),
        };
        let subscription_id = request
            .subscription_id
            .unwrap_or_else(|| Uuid::new_v4().to_string());
        if !valid_id(&subscription_id) {
            return respond(outbound, id, Err(ErrorCode::InvalidMessage));
        }
        if available == 0 && !self.subscriptions.contains_key(&subscription_id) {
            return respond(outbound, id, Err(ErrorCode::ResourceExhausted));
        }
        let cwd = request.cwd;
        let path = request.path;
        let initial_cwd = cwd.clone();
        let initial_path = path.clone();
        let initial = state
            .run(state.files.clone(), ErrorCode::ProjectIo, move |files| {
                Ok(files.filesystem.version(&initial_cwd, &initial_path))
            })
            .await;
        let initial = match initial {
            Ok(initial) => initial,
            Err(error) => return respond(outbound, id, Err(error)),
        };
        self.release(&subscription_id);
        respond(
            outbound,
            id,
            super::encode(wire::SubscribeResult {
                subscription_id: subscription_id.clone(),
                initial: super::project_version(&cwd, &path, initial.clone()),
            }),
        )?;
        if let Some(service) = state.files.clone() {
            let subscription = start_polling(
                Observation {
                    subscription_id: subscription_id.clone(),
                    cwd,
                    path,
                    initial,
                    service,
                },
                state,
                outbound,
            );
            self.subscriptions.insert(subscription_id, subscription);
        }
        Ok(())
    }

    /// Apply one upload frame to an upload owned by this connection.
    /// # Errors
    /// Returns response delivery failures; upload errors use the request error envelope.
    pub async fn frame(
        &mut self,
        id: String,
        frame: FileFrame,
        state: &Shared,
        outbound: &Outbound,
    ) -> Result<(), QueueError> {
        self.prune_uploads();
        let Some(upload) = self.uploads.take(&id) else {
            return respond(outbound, id, Err(ErrorCode::InvalidMessage));
        };
        let result = state
            .run(state.files.clone(), ErrorCode::ProjectIo, move |files| {
                Ok(upload.apply(frame, files))
            })
            .await;
        match result {
            Ok(Ok(UploadStep::Pending(upload))) => {
                self.uploads.resume(id, upload);
                Ok(())
            }
            Ok(Ok(UploadStep::Complete(file))) => respond(
                outbound,
                id,
                super::encode(wire::UploadResult {
                    file: Some(wire::UploadedAttachment::UploadedFile(wire::UploadedFile {
                        id: file.id,
                        file_name: file.file_name,
                        mime_type: file.mime_type,
                        size: file.size,
                        path: file.path,
                    })),
                    error: None,
                }),
            ),
            Ok(Err(error)) => respond(
                outbound,
                id,
                super::encode(wire::UploadResult {
                    file: None,
                    error: Some(error.0),
                }),
            ),
            Err(error) => respond(outbound, id, Err(error)),
        }
    }
}

pub(crate) struct FileRequest {
    pub id: String,
    pub method: String,
    pub params: Value,
    pub available_subscriptions: usize,
}

pub(crate) fn respond(
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

struct Observation {
    subscription_id: String,
    cwd: String,
    path: String,
    initial: super::port::FileVersion,
    service: Arc<Mutex<Files>>,
}

fn start_polling(observation: Observation, state: &Shared, outbound: &Outbound) -> Subscription {
    let Observation {
        subscription_id,
        cwd,
        path,
        initial,
        service,
    } = observation;
    let subscription = Subscription {
        active: Arc::new(Mutex::new(true)),
        cancel: CancellationToken::new(),
    };
    let guard = subscription.active.clone();
    let cancel = subscription.cancel.clone();
    let server_cancel = state.cancellation.clone();
    let outbound = outbound.clone();
    let poll_id = subscription_id;
    let tracker = state.tasks.clone();
    let jobs = state.jobs.clone();
    state.tasks.spawn(async move {
        let mut observation =
            crate::rpc::files::FileObservation::new(cwd.clone(), path.clone(), initial);
        let period = Duration::from_millis(200);
        let mut interval = tokio::time::interval_at(tokio::time::Instant::now() + period, period);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! { biased;
                () = cancel.cancelled() => break,
                () = server_cancel.cancelled() => break,
                () = outbound.failure().cancelled_owned() => break,
                _ = interval.tick() => {},
            }
            let Ok(permit) = jobs.clone().try_acquire_owned() else {
                continue;
            };
            let service = service.clone();
            let cwd_read = cwd.clone();
            let path_read = path.clone();
            let tracking = tracker.token();
            let next = tokio::task::spawn_blocking(move || {
                let (_permit, _tracking) = (permit, tracking);
                let files = service.lock().ok()?;
                Some(files.filesystem.version(&cwd_read, &path_read))
            })
            .await;
            let Ok(Some(next)) = next else {
                continue;
            };
            let Some(version) = observation.update(next) else {
                continue;
            };
            let active = guard
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if !*active {
                break;
            }
            let Ok(params) = super::encode(wire::FileUpdate {
                subscription_id: poll_id.clone(),
                version,
            }) else {
                break;
            };
            if outbound
                .send(&ServerMessage::Event {
                    method: "fs.file.update".to_owned(),
                    params,
                })
                .is_err()
            {
                break;
            }
        }
    });
    subscription
}
