use std::{
    collections::{HashMap, HashSet},
    hash::{DefaultHasher, Hash as _, Hasher as _},
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use ait_contracts::RunView;
use ait_ports::{
    ControlStore, PendingEvent, ProgressCheckpoint, WorkspaceOperation, WorkspaceProgressEvent,
    WorkspaceProgressReporter,
};
use async_trait::async_trait;
use serde_json::{Value, json};
use tokio::{sync::mpsc, task::JoinHandle, time};

const PROGRESS_BUFFER: usize = 256;
const PROGRESS_BATCH: usize = 16;
const PROGRESS_FLUSH_INTERVAL: Duration = Duration::from_millis(40);
const MAX_LIVE_MESSAGE_BYTES: usize = 512 * 1024;
const MAX_PROGRESS_CHECKPOINT_BYTES: usize = 768 * 1024;
const MAX_PROJECTED_ITEMS: usize = 512;
const MAX_WARNINGS: usize = 8;
const MAX_WARNING_BYTES: usize = 8 * 1024;
const MAX_OPERATION_TEXT_BYTES: usize = 64 * 1024;
const MAX_OPERATION_PATHS: usize = 64;
const MAX_OPERATION_PATH_BYTES: usize = 1024;
const MAX_IDENTIFIER_BYTES: usize = 256;

pub(super) struct FinishedProgress {
    pub(super) checkpoint: Option<Value>,
    pub(super) persistence_error: Option<String>,
}

pub(super) struct ProgressPump {
    reporter: Arc<ChannelProgressReporter>,
    writer: JoinHandle<FinishedProgress>,
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

    pub(super) async fn finish(self) -> FinishedProgress {
        drop(self.reporter);
        match self.writer.await {
            Ok(result) => result,
            Err(error) => FinishedProgress {
                checkpoint: None,
                persistence_error: Some(format!("progress writer task failed: {error}")),
            },
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
    dropped_items: usize,
    dropped_warnings: usize,
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
) -> FinishedProgress {
    let mut projection = ProgressProjection {
        status: "running".into(),
        ..ProgressProjection::default()
    };
    let mut persistence_error = None;
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
        projection.enforce_budget(&identity);
        let updated_at = now();
        if let Err(error) = store
            .save_progress(
                ProgressCheckpoint {
                    run_id: identity.run.clone(),
                    body: projection.checkpoint(&identity),
                    updated_at,
                },
                pending,
            )
            .await
        {
            persistence_error = Some(error.to_string());
        }
    }
    projection.enforce_budget(&identity);
    let checkpoint = projection.checkpoint(&identity);
    if let Err(error) = store
        .save_progress(
            ProgressCheckpoint {
                run_id: identity.run.clone(),
                body: checkpoint.clone(),
                updated_at: now(),
            },
            Vec::new(),
        )
        .await
    {
        persistence_error = Some(error.to_string());
    }
    FinishedProgress {
        checkpoint: Some(checkpoint),
        persistence_error,
    }
}

impl ProgressProjection {
    #[allow(
        clippy::too_many_lines,
        reason = "the exhaustive event projection keeps every bounded wire mapping together"
    )]
    fn apply(&mut self, identity: &ProgressIdentity, event: WorkspaceProgressEvent) -> Value {
        match event {
            WorkspaceProgressEvent::MessageStarted { id, phase, text } => {
                let id = bounded_identifier(id);
                self.remember(&id);
                self.items.insert(
                    id.clone(),
                    ProjectedItem::Message {
                        phase: phase.map(|value| bounded_to(value, MAX_IDENTIFIER_BYTES)),
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
                        "phase": self.message_phase(&id),
                        "text": self.message_text(&id),
                    }),
                )
            }
            WorkspaceProgressEvent::TextDelta { id, delta } => {
                let id = bounded_identifier(id);
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
                let id = bounded_identifier(id);
                self.remember(&id);
                let text = bounded(text);
                let phase = phase.map(|value| bounded_to(value, MAX_IDENTIFIER_BYTES));
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
                    "message": bounded_to(message, MAX_WARNING_BYTES),
                    "retrying": retrying,
                    "code": code.map(bounded_identifier),
                });
                if self.warnings.len() == MAX_WARNINGS {
                    self.warnings.remove(0);
                    self.dropped_warnings = self.dropped_warnings.saturating_add(1);
                }
                self.warnings.push(warning.clone());
                envelope(identity, self.seq, "warning", None, &warning)
            }
            WorkspaceProgressEvent::TurnStatus { status, error } => {
                let status = bounded_to(status, MAX_IDENTIFIER_BYTES);
                self.status.clone_from(&status);
                envelope(
                    identity,
                    self.seq,
                    "turn_status",
                    None,
                    &json!({"status": status, "error": error.map(|value| bounded_to(value, MAX_WARNING_BYTES))}),
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
        let operation = bounded_operation(operation);
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
                self.dropped_items = self.dropped_items.saturating_add(1);
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

    fn message_phase(&self, id: &str) -> Option<&str> {
        match self.items.get(id) {
            Some(ProjectedItem::Message { phase, .. }) => phase.as_deref(),
            _ => None,
        }
    }

    fn enforce_budget(&mut self, identity: &ProgressIdentity) {
        while serde_json::to_vec(&self.checkpoint(identity)).map_or(usize::MAX, |body| body.len())
            > MAX_PROGRESS_CHECKPOINT_BYTES
        {
            if let Some(evicted) = self.order.first().cloned() {
                self.order.remove(0);
                self.seen.remove(&evicted);
                self.items.remove(&evicted);
                self.dropped_items = self.dropped_items.saturating_add(1);
            } else if !self.warnings.is_empty() {
                self.warnings.remove(0);
                self.dropped_warnings = self.dropped_warnings.saturating_add(1);
            } else {
                self.status = bounded_to(self.status.clone(), 32);
                break;
            }
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
            "truncated": self.dropped_items > 0 || self.dropped_warnings > 0,
            "dropped_items": self.dropped_items,
            "dropped_warnings": self.dropped_warnings,
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

fn bounded_operation(mut operation: WorkspaceOperation) -> WorkspaceOperation {
    operation.id = bounded_identifier(operation.id);
    operation.kind = bounded_to(operation.kind, MAX_IDENTIFIER_BYTES);
    operation.status = bounded_to(operation.status, MAX_IDENTIFIER_BYTES);
    operation.title = bounded_to(operation.title, MAX_OPERATION_TEXT_BYTES);
    operation.summary = operation
        .summary
        .map(|value| bounded_to(value, MAX_OPERATION_TEXT_BYTES));
    operation.detail = operation
        .detail
        .map(|value| bounded_to(value, MAX_OPERATION_TEXT_BYTES));
    operation.paths = operation
        .paths
        .into_iter()
        .take(MAX_OPERATION_PATHS)
        .map(|value| bounded_to(value, MAX_OPERATION_PATH_BYTES))
        .collect();
    operation
}

fn bounded_identifier(value: String) -> String {
    if value.len() <= MAX_IDENTIFIER_BYTES {
        return value;
    }
    let mut hasher = DefaultHasher::new();
    value.hash(&mut hasher);
    let suffix = format!("#{:016x}", hasher.finish());
    let prefix_limit = MAX_IDENTIFIER_BYTES.saturating_sub(suffix.len());
    format!("{}{}", bounded_to(value, prefix_limit), suffix)
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

    #[test]
    fn one_run_checkpoint_has_a_total_budget_and_bounds_operation_fields() {
        let identity = ProgressIdentity {
            run: "run".into(),
            project: "project".into(),
            session: Some("session".into()),
        };
        let mut projection = ProgressProjection::default();
        for index in 0..32 {
            projection.seq += 1;
            projection.apply(
                &identity,
                WorkspaceProgressEvent::OperationCompleted(WorkspaceOperation {
                    id: format!("operation-{index}-{}", "i".repeat(512)),
                    kind: "k".repeat(512),
                    status: "s".repeat(512),
                    title: "t".repeat(MAX_OPERATION_TEXT_BYTES * 2),
                    summary: Some("s".repeat(MAX_OPERATION_TEXT_BYTES * 2)),
                    detail: Some("d".repeat(MAX_OPERATION_TEXT_BYTES * 2)),
                    paths: (0..MAX_OPERATION_PATHS * 2)
                        .map(|path| format!("{path}-{}", "p".repeat(MAX_OPERATION_PATH_BYTES * 2)))
                        .collect(),
                }),
            );
            projection.enforce_budget(&identity);
        }

        let checkpoint = projection.checkpoint(&identity);
        assert!(serde_json::to_vec(&checkpoint).unwrap().len() <= MAX_PROGRESS_CHECKPOINT_BYTES);
        assert_eq!(checkpoint["truncated"], true);
        assert!(checkpoint["dropped_items"].as_u64().unwrap() > 0);
        let operation = &checkpoint["items"].as_array().unwrap()[0]["operation"];
        assert!(operation["id"].as_str().unwrap().len() <= MAX_IDENTIFIER_BYTES);
        assert!(operation["detail"].as_str().unwrap().len() <= MAX_OPERATION_TEXT_BYTES);
        assert!(operation["paths"].as_array().unwrap().len() <= MAX_OPERATION_PATHS);
    }
}
