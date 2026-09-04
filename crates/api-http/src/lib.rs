//! Local HTTP transport around the shared application service.

use std::{
    collections::BTreeMap,
    convert::Infallible,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Instant, SystemTime, UNIX_EPOCH},
};

use ait_application::LocalControlService;
use ait_contracts::{ApiError, Command, CommandResult, Event as ControlEvent, Response};
use ait_domain::ErrorCode;
use ait_observability::{Correlation, Level, LogRecord, MetricPoint, Telemetry};
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
    router_with_telemetry(service, Telemetry::stderr())
}

/// Builds the version-one router with an injectable observability sink.
pub fn router_with_telemetry(service: Arc<LocalControlService>, telemetry: Telemetry) -> Router {
    Router::new()
        .route("/v1/commands", post(command))
        .route("/v1/events", get(events))
        .route("/v1/metrics", get(metrics))
        .with_state(ApiState { service, telemetry })
}

#[derive(Clone)]
struct ApiState {
    service: Arc<LocalControlService>,
    telemetry: Telemetry,
}

async fn command(State(state): State<ApiState>, Json(command): Json<Command>) -> Json<Response> {
    let started = Instant::now();
    let command_name = command_name(&command);
    let mut correlation = correlation_for_command(&command);
    let response = state.service.execute(command).await;
    enrich_correlation(&mut correlation, &response);
    let elapsed = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    state
        .telemetry
        .metrics()
        .increment("api_commands_total", correlation.clone(), 1);
    state.telemetry.metrics().increment(
        "api_command_duration_ms_total",
        correlation.clone(),
        elapsed,
    );
    if !response.ok {
        state
            .telemetry
            .metrics()
            .increment("api_command_errors_total", correlation.clone(), 1);
    }
    state.telemetry.emit(&LogRecord {
        timestamp_ms: now(),
        level: if response.ok {
            Level::Info
        } else {
            Level::Warn
        },
        target: "ait_api_http".into(),
        event: "command.completed".into(),
        correlation,
        fields: BTreeMap::from([
            ("command".into(), command_name.into()),
            ("duration_ms".into(), elapsed.into()),
            ("ok".into(), response.ok.into()),
        ]),
    });
    Json(response)
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
    State(state): State<ApiState>,
    Query(query): Query<EventQuery>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let events = match state.service.replay_events(query.after, query.limit).await {
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

async fn metrics(State(state): State<ApiState>) -> Json<Vec<MetricPoint>> {
    Json(state.telemetry.metrics().snapshot())
}

fn correlation_for_command(command: &Command) -> Correlation {
    static NEXT_CALL_ID: AtomicU64 = AtomicU64::new(1);
    let call_id = format!("api-{}", NEXT_CALL_ID.fetch_add(1, Ordering::Relaxed));
    let mut correlation = Correlation {
        call_id: Some(call_id),
        ..Correlation::default()
    };
    match command {
        Command::RegisterProject { id, .. } => correlation.project_id = Some(id.clone()),
        Command::CreateSession { id, project_id, .. } => {
            correlation.project_id = Some(project_id.clone());
            correlation.session_id = Some(id.clone());
        }
        Command::SendMessage { session_id, .. } => {
            correlation.session_id = Some(session_id.clone());
        }
        Command::GetRun { run_id } | Command::CancelRun { run_id } => {
            correlation.run_id = Some(run_id.clone());
        }
        Command::CreateCron { project_id, .. } | Command::ExportProject { project_id } => {
            correlation.project_id = Some(project_id.clone());
        }
        Command::ImportProject { archive, .. } => {
            correlation.project_id = Some(archive.project.id.clone());
        }
        Command::RegisterAgent { .. }
        | Command::SetCronEnabled { .. }
        | Command::TriggerCron { .. }
        | Command::Snapshot => {}
    }
    correlation
}

fn enrich_correlation(correlation: &mut Correlation, response: &Response) {
    match response.result.as_ref() {
        Some(CommandResult::Project(project)) => {
            correlation
                .project_id
                .get_or_insert_with(|| project.id.clone());
        }
        Some(CommandResult::Session(session)) => {
            correlation
                .project_id
                .get_or_insert_with(|| session.project_id.clone());
            correlation
                .session_id
                .get_or_insert_with(|| session.id.clone());
        }
        Some(CommandResult::Run(run)) => {
            correlation
                .project_id
                .get_or_insert_with(|| run.project_id.clone());
            if let Some(session_id) = &run.session_id {
                correlation
                    .session_id
                    .get_or_insert_with(|| session_id.clone());
            }
            correlation.run_id.get_or_insert_with(|| run.id.clone());
        }
        Some(CommandResult::ProjectExport(archive)) => {
            correlation
                .project_id
                .get_or_insert_with(|| archive.project.id.clone());
        }
        Some(CommandResult::Agent(_) | CommandResult::Cron(_) | CommandResult::Workspace(_))
        | None => {}
    }
}

const fn command_name(command: &Command) -> &'static str {
    match command {
        Command::RegisterProject { .. } => "register_project",
        Command::RegisterAgent { .. } => "register_agent",
        Command::CreateSession { .. } => "create_session",
        Command::SendMessage { .. } => "send_message",
        Command::GetRun { .. } => "get_run",
        Command::CancelRun { .. } => "cancel_run",
        Command::CreateCron { .. } => "create_cron",
        Command::SetCronEnabled { .. } => "set_cron_enabled",
        Command::TriggerCron { .. } => "trigger_cron",
        Command::ExportProject { .. } => "export_project",
        Command::ImportProject { .. } => "import_project",
        Command::Snapshot => "snapshot",
    }
}

fn now() -> i64 {
    i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
    )
    .unwrap_or(i64::MAX)
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
