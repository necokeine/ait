//! Durable event projection, replay and progress checkpoint reads.
use crate::control::LocalControlService;
use crate::control::errors::store_error;
use ait_contracts::{API_VERSION, ApiError, Event, EventPage};
use ait_ports::PendingEvent;
use serde::Serialize;
use serde_json::Value;
use std::time::{SystemTime, UNIX_EPOCH};

pub(in crate::control) fn pending<T: Serialize>(
    kind: &str,
    entity_id: Option<String>,
    body: &T,
) -> PendingEvent {
    PendingEvent {
        kind: kind.into(),
        entity_id,
        body: {
            let mut value = serde_json::to_value(body).unwrap_or(Value::Null);
            if kind.starts_with("run.")
                && let Some(object) = value.as_object_mut()
            {
                object.remove("execution");
            }
            value
        },
        created_at: now(),
    }
}

pub(in crate::control) fn now() -> i64 {
    i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
    )
    .unwrap_or(i64::MAX)
}

impl LocalControlService {
    /// Replays durable events after a cursor, allowing lossless reconnection.
    ///
    /// # Errors
    ///
    /// Returns a stable recovery error when persistence cannot replay the outbox.
    pub async fn replay_events(&self, after: u64, limit: usize) -> Result<Vec<Event>, ApiError> {
        self.store
            .replay(after, limit.clamp(1, 1_000))
            .await
            .map_err(store_error)
            .map(|events| {
                events
                    .into_iter()
                    .map(|event| Event {
                        api_version: API_VERSION,
                        cursor: event.cursor,
                        kind: event.kind,
                        entity_id: event.entity_id,
                        body: event.body,
                        created_at: event.created_at,
                    })
                    .collect()
            })
    }

    /// Replays a page and reports whether a nonzero reconnect cursor is still
    /// inside the retained event window.
    ///
    /// # Errors
    ///
    /// Returns a stable persistence error when bounds or events cannot be read.
    pub async fn event_page(&self, after: u64, limit: usize) -> Result<EventPage, ApiError> {
        let page = self
            .store
            .replay_page(after, limit.clamp(1, 1_000))
            .await
            .map_err(store_error)?;
        Ok(EventPage {
            events: page
                .events
                .into_iter()
                .map(|event| Event {
                    api_version: API_VERSION,
                    cursor: event.cursor,
                    kind: event.kind,
                    entity_id: event.entity_id,
                    body: event.body,
                    created_at: event.created_at,
                })
                .collect(),
            oldest_cursor: page.bounds.oldest,
            latest_cursor: page.bounds.latest,
            cursor_valid: page.cursor_valid,
        })
    }

    /// Returns bounded live projections used after a refresh or cursor reset.
    ///
    /// # Errors
    ///
    /// Returns a stable persistence error when checkpoints cannot be read.
    pub async fn progress_checkpoints(&self, project_id: &str) -> Result<Vec<Value>, ApiError> {
        self.store
            .load_progress(project_id)
            .await
            .map_err(store_error)
            .map(|values| values.into_iter().map(|value| value.body).collect())
    }
}
