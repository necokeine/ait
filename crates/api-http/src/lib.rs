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
use ait_contracts::{
    AgentMode, ApiError, Command, CommandResult, Event as ControlEvent, ProjectExport,
    ReasoningEffort, Response, SettingsDocument,
};
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
        .route("/v1/project/register", post(register_project))
        .route(
            "/v1/project/set-default-agent",
            post(set_project_default_agent),
        )
        .route("/v1/project/export", post(export_project))
        .route("/v1/project/import", post(import_project))
        .route("/v1/agent/register", post(register_agent))
        .route("/v1/session/create", post(create_session))
        .route("/v1/session/set-agent", post(set_session_agent))
        .route("/v1/session/rename", post(rename_session))
        .route("/v1/session/set-title", post(set_session_title))
        .route("/v1/session/generate-title", post(generate_session_title))
        .route("/v1/session/send-message", post(send_message))
        .route("/v1/session/fork", post(fork_session))
        .route("/v1/run/get", post(get_run))
        .route("/v1/run/cancel", post(cancel_run))
        .route("/v1/cron/create", post(create_cron))
        .route("/v1/cron/set-enabled", post(set_cron_enabled))
        .route("/v1/cron/trigger", post(trigger_cron))
        .route("/v1/workspace/snapshot", get(workspace_snapshot))
        .route("/v1/settings", get(get_settings))
        .route("/v1/settings/save", post(save_settings))
        .route("/v1/settings/reset", post(reset_settings))
        .route("/v1/event/list", get(events))
        .route("/v1/metric/list", get(metrics))
        .with_state(ApiState { service, telemetry })
}

#[derive(Clone)]
struct ApiState {
    service: Arc<LocalControlService>,
    telemetry: Telemetry,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RegisterProjectRequest {
    id: String,
    name: String,
    workdir: String,
    #[serde(default)]
    repo_url: Option<String>,
}

async fn register_project(
    State(state): State<ApiState>,
    Json(request): Json<RegisterProjectRequest>,
) -> Json<Response> {
    execute_command(
        state,
        Command::RegisterProject {
            id: request.id,
            name: request.name,
            workdir: request.workdir,
            repo_url: request.repo_url,
        },
    )
    .await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SetProjectDefaultAgentRequest {
    project_id: String,
    agent_id: String,
}

async fn set_project_default_agent(
    State(state): State<ApiState>,
    Json(request): Json<SetProjectDefaultAgentRequest>,
) -> Json<Response> {
    execute_command(
        state,
        Command::SetProjectDefaultAgent {
            project_id: request.project_id,
            agent_id: request.agent_id,
        },
    )
    .await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ExportProjectRequest {
    project_id: String,
}

async fn export_project(
    State(state): State<ApiState>,
    Json(request): Json<ExportProjectRequest>,
) -> Json<Response> {
    execute_command(
        state,
        Command::ExportProject {
            project_id: request.project_id,
        },
    )
    .await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ImportProjectRequest {
    archive: ProjectExport,
    workdir: String,
}

async fn import_project(
    State(state): State<ApiState>,
    Json(request): Json<ImportProjectRequest>,
) -> Json<Response> {
    execute_command(
        state,
        Command::ImportProject {
            archive: request.archive,
            workdir: request.workdir,
        },
    )
    .await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RegisterAgentRequest {
    id: String,
    name: String,
    model: String,
    #[serde(default = "default_agent_mode")]
    mode: AgentMode,
}

const fn default_agent_mode() -> AgentMode {
    AgentMode::Echo
}

async fn register_agent(
    State(state): State<ApiState>,
    Json(request): Json<RegisterAgentRequest>,
) -> Json<Response> {
    execute_command(
        state,
        Command::RegisterAgent {
            id: request.id,
            name: request.name,
            model: request.model,
            mode: request.mode,
        },
    )
    .await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateSessionRequest {
    id: String,
    project_id: String,
    agent_id: String,
    #[serde(default)]
    at_message_id: Option<String>,
}

async fn create_session(
    State(state): State<ApiState>,
    Json(request): Json<CreateSessionRequest>,
) -> Json<Response> {
    execute_command(
        state,
        Command::CreateSession {
            id: request.id,
            project_id: request.project_id,
            agent_id: request.agent_id,
            at_message_id: request.at_message_id,
        },
    )
    .await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SetSessionAgentRequest {
    session_id: String,
    agent_id: String,
    #[serde(default)]
    expected_version: Option<u64>,
}

async fn set_session_agent(
    State(state): State<ApiState>,
    Json(request): Json<SetSessionAgentRequest>,
) -> Json<Response> {
    execute_command(
        state,
        Command::SetSessionAgent {
            session_id: request.session_id,
            agent_id: request.agent_id,
            expected_version: request.expected_version,
        },
    )
    .await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RenameSessionRequest {
    session_id: String,
    name: String,
}

async fn rename_session(
    State(state): State<ApiState>,
    Json(request): Json<RenameSessionRequest>,
) -> Json<Response> {
    execute_command(
        state,
        Command::RenameSession {
            session_id: request.session_id,
            name: request.name,
        },
    )
    .await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SetSessionTitleRequest {
    session_id: String,
    title: String,
}

async fn set_session_title(
    State(state): State<ApiState>,
    Json(request): Json<SetSessionTitleRequest>,
) -> Json<Response> {
    execute_command(
        state,
        Command::SetSessionTitle {
            session_id: request.session_id,
            title: request.title,
        },
    )
    .await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GenerateSessionTitleRequest {
    session_id: String,
    prompt: String,
}

async fn generate_session_title(
    State(state): State<ApiState>,
    Json(request): Json<GenerateSessionTitleRequest>,
) -> Json<Response> {
    Json(
        state
            .service
            .generate_session_title(request.session_id, request.prompt)
            .await,
    )
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SendMessageRequest {
    session_id: String,
    text: String,
    #[serde(default)]
    expected_version: Option<u64>,
    #[serde(default)]
    reasoning_effort: Option<ReasoningEffort>,
}

async fn send_message(
    State(state): State<ApiState>,
    Json(request): Json<SendMessageRequest>,
) -> Json<Response> {
    execute_command(
        state,
        Command::SendMessage {
            session_id: request.session_id,
            text: request.text,
            expected_version: request.expected_version,
            reasoning_effort: request.reasoning_effort,
        },
    )
    .await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ForkSessionRequest {
    id: String,
    project_id: String,
    agent_id: String,
    at_message_id: String,
    text: String,
    #[serde(default)]
    reasoning_effort: Option<ReasoningEffort>,
}

async fn fork_session(
    State(state): State<ApiState>,
    Json(request): Json<ForkSessionRequest>,
) -> Json<Response> {
    execute_command(
        state,
        Command::ForkSession {
            id: request.id,
            project_id: request.project_id,
            agent_id: request.agent_id,
            at_message_id: request.at_message_id,
            text: request.text,
            reasoning_effort: request.reasoning_effort,
        },
    )
    .await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RunRequest {
    run_id: String,
}

async fn get_run(State(state): State<ApiState>, Json(request): Json<RunRequest>) -> Json<Response> {
    execute_command(
        state,
        Command::GetRun {
            run_id: request.run_id,
        },
    )
    .await
}

async fn cancel_run(
    State(state): State<ApiState>,
    Json(request): Json<RunRequest>,
) -> Json<Response> {
    execute_command(
        state,
        Command::CancelRun {
            run_id: request.run_id,
        },
    )
    .await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateCronRequest {
    id: String,
    name: String,
    project_id: String,
    base_message_id: String,
    agent_id: String,
    schedule: String,
    timezone: String,
}

async fn create_cron(
    State(state): State<ApiState>,
    Json(request): Json<CreateCronRequest>,
) -> Json<Response> {
    execute_command(
        state,
        Command::CreateCron {
            id: request.id,
            name: request.name,
            project_id: request.project_id,
            base_message_id: request.base_message_id,
            agent_id: request.agent_id,
            schedule: request.schedule,
            timezone: request.timezone,
        },
    )
    .await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SetCronEnabledRequest {
    cron_id: String,
    enabled: bool,
}

async fn set_cron_enabled(
    State(state): State<ApiState>,
    Json(request): Json<SetCronEnabledRequest>,
) -> Json<Response> {
    execute_command(
        state,
        Command::SetCronEnabled {
            cron_id: request.cron_id,
            enabled: request.enabled,
        },
    )
    .await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TriggerCronRequest {
    cron_id: String,
    scheduled_at: i64,
}

async fn trigger_cron(
    State(state): State<ApiState>,
    Json(request): Json<TriggerCronRequest>,
) -> Json<Response> {
    execute_command(
        state,
        Command::TriggerCron {
            cron_id: request.cron_id,
            scheduled_at: request.scheduled_at,
        },
    )
    .await
}

async fn workspace_snapshot(State(state): State<ApiState>) -> Json<Response> {
    execute_command(state, Command::Snapshot).await
}

async fn get_settings(State(state): State<ApiState>) -> Json<Response> {
    execute_command(state, Command::GetSettings).await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SaveSettingsRequest {
    expected_revision: u64,
    values: SettingsDocument,
}

async fn save_settings(
    State(state): State<ApiState>,
    Json(request): Json<SaveSettingsRequest>,
) -> Json<Response> {
    execute_command(
        state,
        Command::SaveSettings {
            expected_revision: request.expected_revision,
            values: request.values,
        },
    )
    .await
}

async fn reset_settings(State(state): State<ApiState>) -> Json<Response> {
    execute_command(state, Command::ResetSettings).await
}

async fn execute_command(state: ApiState, command: Command) -> Json<Response> {
    let started = Instant::now();
    let operation_name = operation_name(&command);
    let mut correlation = correlation_for_command(&command);
    let response = state.service.execute(command).await;
    enrich_correlation(&mut correlation, &response);
    let elapsed = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    state
        .telemetry
        .metrics()
        .increment("api_operations_total", correlation.clone(), 1);
    state.telemetry.metrics().increment(
        "api_operation_duration_ms_total",
        correlation.clone(),
        elapsed,
    );
    if !response.ok {
        state
            .telemetry
            .metrics()
            .increment("api_operation_errors_total", correlation.clone(), 1);
    }
    state.telemetry.emit(&LogRecord {
        timestamp_ms: now(),
        level: if response.ok {
            Level::Info
        } else {
            Level::Warn
        },
        target: "ait_api_http".into(),
        event: "operation.completed".into(),
        correlation,
        fields: BTreeMap::from([
            ("operation".into(), operation_name.into()),
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
        Command::CreateSession { id, project_id, .. }
        | Command::ForkSession { id, project_id, .. } => {
            correlation.project_id = Some(project_id.clone());
            correlation.session_id = Some(id.clone());
        }
        Command::SetSessionAgent { session_id, .. }
        | Command::RenameSession { session_id, .. }
        | Command::SetSessionTitle { session_id, .. }
        | Command::SendMessage { session_id, .. } => {
            correlation.session_id = Some(session_id.clone());
        }
        Command::GetRun { run_id } | Command::CancelRun { run_id } => {
            correlation.run_id = Some(run_id.clone());
        }
        Command::SetProjectDefaultAgent { project_id, .. }
        | Command::CreateCron { project_id, .. }
        | Command::ExportProject { project_id } => {
            correlation.project_id = Some(project_id.clone());
        }
        Command::ImportProject { archive, .. } => {
            correlation.project_id = Some(archive.project.id.clone());
        }
        Command::RegisterAgent { .. }
        | Command::SetCronEnabled { .. }
        | Command::TriggerCron { .. }
        | Command::GetSettings
        | Command::SaveSettings { .. }
        | Command::ResetSettings
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
        Some(
            CommandResult::Agent(_)
            | CommandResult::Cron(_)
            | CommandResult::Settings(_)
            | CommandResult::Workspace(_),
        )
        | None => {}
    }
}

const fn operation_name(command: &Command) -> &'static str {
    match command {
        Command::RegisterProject { .. } => "register_project",
        Command::SetProjectDefaultAgent { .. } => "set_project_default_agent",
        Command::RegisterAgent { .. } => "register_agent",
        Command::CreateSession { .. } => "create_session",
        Command::SetSessionAgent { .. } => "set_session_agent",
        Command::RenameSession { .. } => "rename_session",
        Command::SetSessionTitle { .. } => "set_session_title",
        Command::SendMessage { .. } => "send_message",
        Command::ForkSession { .. } => "fork_session",
        Command::GetRun { .. } => "get_run",
        Command::CancelRun { .. } => "cancel_run",
        Command::CreateCron { .. } => "create_cron",
        Command::SetCronEnabled { .. } => "set_cron_enabled",
        Command::TriggerCron { .. } => "trigger_cron",
        Command::ExportProject { .. } => "export_project",
        Command::ImportProject { .. } => "import_project",
        Command::GetSettings => "get_settings",
        Command::SaveSettings { .. } => "save_settings",
        Command::ResetSettings => "reset_settings",
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
