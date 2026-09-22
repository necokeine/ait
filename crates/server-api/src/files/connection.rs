use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use server_application::files::{FileUpload, Files, UploadedFile};
use server_protocol::{
    ErrorCode, ServerMessage, file_transfer::FileFrame, files as wire, valid_id,
};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::Shared;
use crate::outbound::{Outbound, QueueError};

#[derive(Default)]
pub(crate) struct FileConnection {
    subscriptions: BTreeMap<String, Subscription>,
    uploads: BTreeMap<String, Upload>,
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

struct Upload {
    metadata: UploadedFile,
    writer: Option<Box<dyn FileUpload>>,
    touched: Instant,
}

impl FileConnection {
    pub(crate) fn len(&self) -> usize {
        self.subscriptions.len()
    }

    pub(crate) fn release(&mut self, id: &str) {
        self.subscriptions.remove(id);
    }

    pub(crate) fn prune_uploads(&mut self) {
        self.uploads
            .retain(|_, upload| upload.touched.elapsed() < Duration::from_secs(600));
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
            "file.upload.request" => match self.begin_upload(&id, params) {
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
        let initial = crate::jobs::run(
            state,
            state.files.clone(),
            ErrorCode::ProjectIo,
            move |files| Ok(files.filesystem.version(&initial_cwd, &initial_path)),
        )
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

    fn begin_upload(&mut self, id: &str, params: Value) -> Result<(), ErrorCode> {
        let request: wire::UploadRequest = super::decode(params)?;
        self.prune_uploads();
        if request.file_name.is_empty()
            || request.mime_type.is_empty()
            || request.file_name.len() > 255
        {
            return Err(ErrorCode::InvalidMessage);
        }
        if request.size > 64 * 1024 * 1024
            || (self.uploads.len() >= 8 && !self.uploads.contains_key(id))
        {
            return Err(ErrorCode::ResourceExhausted);
        }
        self.uploads.insert(
            id.to_owned(),
            Upload {
                metadata: UploadedFile {
                    id: format!("upload_{}", Uuid::new_v4()),
                    file_name: request.file_name,
                    mime_type: request.mime_type,
                    size: request.size,
                    path: String::new(),
                },
                writer: None,
                touched: Instant::now(),
            },
        );
        Ok(())
    }

    pub(crate) async fn frame(
        &mut self,
        id: String,
        frame: FileFrame,
        state: &Shared,
        outbound: &Outbound,
    ) -> Result<(), QueueError> {
        self.prune_uploads();
        let Some(mut upload) = self.uploads.remove(&id) else {
            return respond(outbound, id, Err(ErrorCode::InvalidMessage));
        };
        upload.touched = Instant::now();
        let result = crate::jobs::run(
            state,
            state.files.clone(),
            ErrorCode::ProjectIo,
            move |files| Ok(apply_frame(upload, frame, files)),
        )
        .await;
        match result {
            Ok(Ok(UploadStep::Pending(upload))) => {
                self.uploads.insert(id, upload);
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

enum UploadStep {
    Pending(Upload),
    Complete(UploadedFile),
}

fn apply_frame(
    mut upload: Upload,
    frame: FileFrame,
    files: &Files,
) -> Result<UploadStep, super::port::FileError> {
    use std::io::Write;
    match frame {
        FileFrame::Begin(_) => {
            if upload.writer.is_some() {
                return Err(super::port::FileError("Upload already started".to_owned()));
            }
            upload.writer = Some(files.filesystem.upload(upload.metadata.clone())?);
        }
        FileFrame::Chunk(bytes) => {
            let writer = upload.writer.as_mut().ok_or_else(|| {
                super::port::FileError("Upload chunks arrived before file begin.".to_owned())
            })?;
            writer.write_all(&bytes)?;
        }
        FileFrame::End => {
            let writer = upload.writer.take().ok_or_else(|| {
                super::port::FileError("Upload ended before file begin.".to_owned())
            })?;
            return Ok(UploadStep::Complete(writer.finish()?));
        }
    }
    Ok(UploadStep::Pending(upload))
}

pub(crate) struct FileRequest {
    pub id: String,
    pub method: String,
    pub params: Value,
    pub available_subscriptions: usize,
}

pub(super) fn respond(
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
        let mut previous = initial;
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
            if next == previous {
                continue;
            }
            previous = next.clone();
            let active = guard
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if !*active {
                break;
            }
            let Ok(params) = super::encode(wire::FileUpdate {
                subscription_id: poll_id.clone(),
                version: super::project_version(&cwd, &path, next),
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

#[cfg(test)]
mod tests;
