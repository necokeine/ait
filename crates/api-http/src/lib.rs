//! Local HTTP transport around the shared application service.

use std::{convert::Infallible, sync::Arc};

use ait_application::LocalControlService;
use ait_contracts::{ApiError, Command, Event as ControlEvent, Response};
use ait_domain::ErrorCode;
use axum::{
    Json, Router,
    extract::{Query, State},
    response::sse::{Event, KeepAlive, Sse},
    routing::{get, post},
};
use serde::Deserialize;
use tokio_stream::{self as stream, Stream};

/// Builds the version-one local API router.
pub fn router(service: Arc<LocalControlService>) -> Router {
    Router::new()
        .route("/v1/commands", post(command))
        .route("/v1/events", get(events))
        .with_state(service)
}

async fn command(
    State(service): State<Arc<LocalControlService>>,
    Json(command): Json<Command>,
) -> Json<Response> {
    Json(service.execute(command).await)
}

#[derive(Deserialize)]
struct EventQuery {
    #[serde(default)]
    after: u64,
    #[serde(default = "default_limit")]
    limit: usize,
}

const fn default_limit() -> usize {
    256
}

async fn events(
    State(service): State<Arc<LocalControlService>>,
    Query(query): Query<EventQuery>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let events = match service.replay_events(query.after, query.limit).await {
        Ok(events) => events,
        Err(error) => vec![error_event(error)],
    };
    let output = events.into_iter().map(|event| {
        let data = serde_json::to_string(&event).unwrap_or_else(|_| "{}".into());
        Ok(Event::default()
            .id(event.cursor.to_string())
            .event(event.kind)
            .data(data))
    });
    Sse::new(stream::iter(output)).keep_alive(KeepAlive::default())
}

fn error_event(error: ApiError) -> ControlEvent {
    ControlEvent {
        api_version: 1,
        cursor: 0,
        kind: "stream.error".into(),
        entity_id: None,
        body: serde_json::to_value(error).unwrap_or_default(),
        created_at: 0,
    }
}

/// Returns a transport-safe malformed request response.
#[must_use]
pub fn malformed_request(message: impl Into<String>) -> Response {
    Response::failure(ApiError {
        code: ErrorCode::InvalidRun,
        message: message.into(),
        retryable: false,
    })
}
