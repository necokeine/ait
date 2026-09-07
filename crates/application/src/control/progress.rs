use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use ait_contracts::RunView;
use ait_ports::{
    ControlStore, ControlStoreError, PendingEvent, ProgressCheckpoint, WorkspaceOperation,
    WorkspaceProgressEvent, WorkspaceProgressReporter,
};
use async_trait::async_trait;
use serde_json::{Value, json};
use tokio::{sync::mpsc, task::JoinHandle, time};

const PROGRESS_BUFFER: usize = 256;
const PROGRESS_BATCH: usize = 64;
const PROGRESS_FLUSH_INTERVAL: Duration = Duration::from_millis(40);
const PROGRESS_DRAIN_DEADLINE: Duration = Duration::from_millis(500);
const MAX_LIVE_MESSAGE_BYTES: usize = 512 * 1024;
const MAX_PROJECTED_ITEMS: usize = 512;
const MAX_WARNINGS: usize = 8;

pub(super) struct ProgressPump {
    reporter: Arc<ChannelProgressReporter>,
    writer: JoinHandle<Result<(), ControlStoreError>>,
}

impl ProgressPump {
    pub(super) fn start(store: Arc<dyn ControlStore>, run: &RunView) -> Self {
        let (sender, receiver) = mpsc::channel(PROGRESS_BUFFER);
        let identity = ProgressIdentity {
            run: run.id.clone(),
            project: run.project_id.clone(),
            session: run.session_id.clone(),
        };
        let writer = tokio::spawn(write_progress(store, identity, receiver));
        Self {
            reporter: Arc::new(ChannelProgressReporter { sender }),
            writer,
        }
    }

    pub(super) fn reporter(&self) -> Arc<dyn WorkspaceProgressReporter> {
        self.reporter.clone()
    }

    pub(super) async fn finish(self) -> Result<(), ControlStoreError> {
        drop(self.reporter);
        let mut writer = self.writer;
        if let Ok(result) = time::timeout(PROGRESS_DRAIN_DEADLINE, &mut writer).await {
            result.map_err(|error| {
                ControlStoreError::Other(format!("progress writer task failed: {error}"))
            })?
        } else {
            writer.abort();
            Err(ControlStoreError::Other(
                "progress writer did not drain before the terminal deadline".into(),
            ))
        }
    }
}

struct ChannelProgressReporter {
    sender: mpsc::Sender<WorkspaceProgressEvent>,
}

#[async_trait]
impl WorkspaceProgressReporter for ChannelProgressReporter {
    async fn report(&self, event: WorkspaceProgressEvent) {
        // The bounded channel applies backpressure to the adapter without tying
        // execution to any desktop subscriber or client connection.
        let _ = self.sender.send(event).await;
    }
}

#[derive(Clone)]
struct ProgressIdentity {
    run: String,
    project: String,
    session: Option<String>,
}

#[derive(Default)]
struct ProgressProjection {
    seq: u64,
    status: String,
    order: Vec<String>,
    seen: HashSet<String>,
    items: HashMap<String, ProjectedItem>,
    warnings: Vec<Value>,
}

enum ProjectedItem {
    Message {
        phase: Option<String>,
        text: String,
        completed: bool,
    },
    Operation(WorkspaceOperation),
}

async fn write_progress(
    store: Arc<dyn ControlStore>,
    identity: ProgressIdentity,
    mut receiver: mpsc::Receiver<WorkspaceProgressEvent>,
) -> Result<(), ControlStoreError> {
    let mut projection = ProgressProjection {
        status: "running".into(),
        ..ProgressProjection::default()
    };
    while let Some(first) = receiver.recv().await {
        let mut batch = vec![first];
        let deadline = time::Instant::now() + PROGRESS_FLUSH_INTERVAL;
        while batch.len() < PROGRESS_BATCH {
            match time::timeout_at(deadline, receiver.recv()).await {
                Ok(Some(event)) => batch.push(event),
                Ok(None) | Err(_) => break,
            }
        }
        let mut pending = Vec::with_capacity(batch.len());
        for event in batch {
            projection.seq = projection.seq.saturating_add(1);
            let body = projection.apply(&identity, event);
            pending.push(PendingEvent {
                kind: "run.progress".into(),
                entity_id: Some(identity.run.clone()),
                body,
                created_at: now(),
            });
        }
        let updated_at = now();
        store
            .save_progress(
                ProgressCheckpoint {
                    run_id: identity.run.clone(),
                    body: projection.checkpoint(&identity),
                    updated_at,
                },
                pending,
            )
            .await?;
    }
    Ok(())
}

impl ProgressProjection {
    fn apply(&mut self, identity: &ProgressIdentity, event: WorkspaceProgressEvent) -> Value {
        match event {
            WorkspaceProgressEvent::MessageStarted { id, phase, text } => {
                self.remember(&id);
                self.items.insert(
                    id.clone(),
                    ProjectedItem::Message {
                        phase: phase.clone(),
                        text: bounded(text),
                        completed: false,
                    },
                );
                envelope(
                    identity,
                    self.seq,
                    "message_started",
                    Some(&id),
                    &json!({
                        "phase": phase,
                        "text": self.message_text(&id),
                    }),
                )
            }
            WorkspaceProgressEvent::TextDelta { id, delta } => {
                self.remember(&id);
                let item = self
                    .items
                    .entry(id.clone())
                    .or_insert_with(|| ProjectedItem::Message {
                        phase: None,
                        text: String::new(),
                        completed: false,
                    });
                let mut accepted = String::new();
                if let ProjectedItem::Message { text, .. } = item {
                    accepted = bounded_to(delta, MAX_LIVE_MESSAGE_BYTES.saturating_sub(text.len()));
                    text.push_str(&accepted);
                }
                envelope(
                    identity,
                    self.seq,
                    "text_delta",
                    Some(&id),
                    &json!({"delta": accepted}),
                )
            }
            WorkspaceProgressEvent::MessageCompleted { id, phase, text } => {
                self.remember(&id);
                let text = bounded(text);
                self.items.insert(
                    id.clone(),
                    ProjectedItem::Message {
                        phase: phase.clone(),
                        text: text.clone(),
                        completed: true,
                    },
                );
                envelope(
                    identity,
                    self.seq,
                    "message_completed",
                    Some(&id),
                    &json!({"phase": phase, "text": text}),
                )
            }
            WorkspaceProgressEvent::OperationStarted(operation) => {
                self.operation(identity, operation, "operation_started")
            }
            WorkspaceProgressEvent::OperationCompleted(operation) => {
                self.operation(identity, operation, "operation_completed")
            }
            WorkspaceProgressEvent::Warning {
                message,
                retrying,
                code,
            } => {
                let warning = json!({
                    "message": bounded(message),
                    "retrying": retrying,
                    "code": code,
                });
                if self.warnings.len() == MAX_WARNINGS {
                    self.warnings.remove(0);
                }
                self.warnings.push(warning.clone());
                envelope(identity, self.seq, "warning", None, &warning)
            }
            WorkspaceProgressEvent::TurnStatus { status, error } => {
                self.status.clone_from(&status);
                envelope(
                    identity,
                    self.seq,
                    "turn_status",
                    None,
                    &json!({"status": status, "error": error.map(bounded)}),
                )
            }
        }
    }

    fn operation(
        &mut self,
        identity: &ProgressIdentity,
        operation: WorkspaceOperation,
        event_type: &str,
    ) -> Value {
        self.remember(&operation.id);
        let id = operation.id.clone();
        let value = operation_value(&operation);
        self.items
            .insert(id.clone(), ProjectedItem::Operation(operation));
        envelope(
            identity,
            self.seq,
            event_type,
            Some(&id),
            &json!({"operation": value}),
        )
    }

    fn remember(&mut self, id: &str) {
        if self.seen.insert(id.to_owned()) {
            if self.order.len() == MAX_PROJECTED_ITEMS {
                let evicted = self.order.remove(0);
                self.seen.remove(&evicted);
                self.items.remove(&evicted);
            }
            self.order.push(id.to_owned());
        }
    }

    fn message_text(&self, id: &str) -> &str {
        match self.items.get(id) {
            Some(ProjectedItem::Message { text, .. }) => text,
            _ => "",
        }
    }

    fn checkpoint(&self, identity: &ProgressIdentity) -> Value {
        let items = self
            .order
            .iter()
            .filter_map(|id| match self.items.get(id) {
                Some(ProjectedItem::Message {
                    phase,
                    text,
                    completed,
                }) => Some(json!({
                    "type": "message",
                    "id": id,
                    "phase": phase,
                    "text": text,
                    "completed": completed,
                })),
                Some(ProjectedItem::Operation(operation)) => {
                    Some(json!({"type": "operation", "operation": operation_value(operation)}))
                }
                None => None,
            })
            .collect::<Vec<_>>();
        json!({
            "version": 1,
            "run_id": identity.run,
            "project_id": identity.project,
            "session_id": identity.session,
            "seq": self.seq,
            "status": self.status,
            "items": items,
            "warnings": self.warnings,
            "updated_at": now(),
        })
    }
}

fn envelope(
    identity: &ProgressIdentity,
    seq: u64,
    event_type: &str,
    item_id: Option<&str>,
    fields: &Value,
) -> Value {
    let mut value = json!({
        "version": 1,
        "run_id": identity.run,
        "project_id": identity.project,
        "session_id": identity.session,
        "seq": seq,
        "type": event_type,
        "item_id": item_id,
    });
    if let (Some(target), Some(source)) = (value.as_object_mut(), fields.as_object()) {
        target.extend(source.clone());
    }
    value
}

fn operation_value(operation: &WorkspaceOperation) -> Value {
    json!({
        "id": operation.id,
        "kind": operation.kind,
        "status": operation.status,
        "title": operation.title,
        "summary": operation.summary,
        "detail": operation.detail,
        "paths": operation.paths,
    })
}

fn bounded(value: String) -> String {
    bounded_to(value, MAX_LIVE_MESSAGE_BYTES)
}

fn bounded_to(mut value: String, limit: usize) -> String {
    if value.len() <= limit {
        return value;
    }
    let mut boundary = limit;
    while !value.is_char_boundary(boundary) {
        boundary -= 1;
    }
    value.truncate(boundary);
    value
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| {
            i64::try_from(duration.as_millis()).unwrap_or(i64::MAX)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_projection_limits_text_on_a_utf8_boundary() {
        let oversized = "界".repeat(MAX_LIVE_MESSAGE_BYTES / 3 + 2);
        let bounded = bounded(oversized);

        assert!(bounded.len() <= MAX_LIVE_MESSAGE_BYTES);
        assert!(bounded.is_char_boundary(bounded.len()));
        assert_eq!(bounded_to("界".into(), 2), "");
    }

    #[test]
    fn live_projection_evicts_the_oldest_item() {
        let mut projection = ProgressProjection::default();
        for index in 0..=MAX_PROJECTED_ITEMS {
            let id = format!("item-{index}");
            projection.remember(&id);
            projection.items.insert(
                id,
                ProjectedItem::Message {
                    phase: None,
                    text: String::new(),
                    completed: false,
                },
            );
        }

        assert_eq!(projection.order.len(), MAX_PROJECTED_ITEMS);
        assert!(!projection.seen.contains("item-0"));
        assert!(!projection.items.contains_key("item-0"));
        assert_eq!(projection.order.first().map(String::as_str), Some("item-1"));
    }
}
