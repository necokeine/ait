#![allow(missing_docs)]

use std::{
    collections::{HashMap, HashSet},
    fmt::Write as _,
    path::Path,
    process::Command as ProcessCommand,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use ait_contracts::{
    API_VERSION, AgentMode, AgentView, ApiError, Command, CommandResult, CronView, Event,
    MessageView, PROJECT_EXPORT_VERSION, ProjectExport, ProjectView, ReasoningEffort, Response,
    RunView, SessionView, SettingKind, SettingsDocument, SettingsView, WorkspaceView,
    default_settings, settings_schema,
};
use ait_domain::{
    AgentId, Cron, CronConcurrencyPolicy, CronId, CronMisfirePolicy, DomainError, ErrorCode,
    MessageId, ProjectId, TimestampMs,
};
use ait_ports::{
    ControlStore, ControlStoreError, PendingEvent, SessionTitleGenerator, SessionTitleRequest,
    WorkspaceAgent, WorkspaceAgentInvocation, WorkspaceAgentResponse,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct State {
    projects: Vec<ProjectView>,
    agents: Vec<AgentView>,
    sessions: Vec<SessionView>,
    messages: Vec<MessageView>,
    runs: Vec<RunView>,
    crons: Vec<CronView>,
    #[serde(default = "default_settings")]
    settings: SettingsDocument,
    #[serde(default = "default_settings_revision")]
    settings_revision: u64,
}

struct ForkSessionInput {
    id: String,
    project_id: String,
    agent_id: String,
    at_message_id: String,
    text: String,
    reasoning_effort: Option<ReasoningEffort>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            projects: Vec::new(),
            agents: Vec::new(),
            sessions: Vec::new(),
            messages: Vec::new(),
            runs: Vec::new(),
            crons: Vec::new(),
            settings: default_settings(),
            settings_revision: default_settings_revision(),
        }
    }
}

const fn default_settings_revision() -> u64 {
    1
}

impl From<State> for WorkspaceView {
    fn from(state: State) -> Self {
        Self {
            projects: state.projects,
            agents: state.agents,
            sessions: state.sessions,
            messages: state.messages,
            runs: state.runs,
            crons: state.crons,
        }
    }
}

/// Shared application entry point used by every transport adapter.
pub struct LocalControlService {
    store: Arc<dyn ControlStore>,
    workspace_agent: Option<Arc<dyn WorkspaceAgent>>,
    session_title_generator: Option<Arc<dyn SessionTitleGenerator>>,
}

impl LocalControlService {
    #[must_use]
    pub fn new(store: Arc<dyn ControlStore>) -> Self {
        Self {
            store,
            workspace_agent: None,
            session_title_generator: None,
        }
    }

    /// Creates a service that can execute real workspace-scoped coding Agents.
    #[must_use]
    pub fn with_workspace_agent(
        store: Arc<dyn ControlStore>,
        workspace_agent: Arc<dyn WorkspaceAgent>,
    ) -> Self {
        Self {
            store,
            workspace_agent: Some(workspace_agent),
            session_title_generator: None,
        }
    }

    /// Adds the read-only generator used for first-interaction Session metadata.
    #[must_use]
    pub fn with_session_title_generator(
        mut self,
        generator: Arc<dyn SessionTitleGenerator>,
    ) -> Self {
        self.session_title_generator = Some(generator);
        self
    }

    /// Executes one versioned command and returns a stable response envelope.
    pub async fn execute(&self, command: Command) -> Response {
        match self.try_execute(command).await {
            Ok(CommandResult::Run(run)) if run.status == "queued" => {
                match self.execute_workspace_agent(run).await {
                    Ok(result) => Response::success(CommandResult::Run(result)),
                    Err(error) => Response::failure(error),
                }
            }
            Ok(result) => Response::success(result),
            Err(error) => Response::failure(error),
        }
    }

    /// Runs the one-shot background title turn after a Session's first interaction.
    pub async fn generate_session_title(
        &self,
        session_id: String,
        user_prompt: String,
    ) -> Response {
        match self
            .try_generate_session_title(&session_id, &user_prompt)
            .await
        {
            Ok(session) => Response::success(CommandResult::Session(session)),
            Err(error) => Response::failure(error),
        }
    }

    async fn try_generate_session_title(
        &self,
        session_id: &str,
        user_prompt: &str,
    ) -> Result<SessionView, ApiError> {
        let bounded_prompt = user_prompt.chars().take(2_000).collect::<String>();
        if bounded_prompt.trim().is_empty() {
            return Err(error(
                ErrorCode::InvalidSession,
                "Session title prompt is empty",
                false,
            ));
        }
        let (session, workdir, should_generate) = self.begin_title_generation(session_id).await?;
        if !should_generate {
            return Ok(session);
        }
        let generator = self.session_title_generator.as_ref().ok_or_else(|| {
            error(
                ErrorCode::InvalidConfiguration,
                "Session title generator is not configured",
                false,
            )
        })?;
        let generated = generator
            .generate(SessionTitleRequest {
                request_id: format!("session-title-{}", Uuid::new_v4()),
                user_prompt: bounded_prompt,
                cwd: workdir.into(),
                cancellation: tokio_util::sync::CancellationToken::new(),
            })
            .await
            .map_err(|failure| error(failure.code, failure.message, failure.retryable))?;
        validate_session_metadata(&generated.title, &generated.description)?;
        self.finish_title_generation(session_id, generated.title, generated.description)
            .await
    }

    async fn begin_title_generation(
        &self,
        session_id: &str,
    ) -> Result<(SessionView, String, bool), ApiError> {
        for _ in 0..4 {
            let snapshot = self.store.load().await.map_err(store_error)?;
            let mut state = decode_state(snapshot.value)?;
            let index = state
                .sessions
                .iter()
                .position(|session| session.id == session_id)
                .ok_or_else(|| error(ErrorCode::SessionNotFound, "session not found", false))?;
            let session = state.sessions[index].clone();
            let project = state
                .projects
                .iter()
                .find(|project| project.id == session.project_id)
                .ok_or_else(|| error(ErrorCode::InvalidProject, "project not found", false))?;
            if session.title_generation_started || !session.name.trim().is_empty() {
                return Ok((session, project.workdir.clone(), false));
            }
            if !is_first_completed_interaction(&state, &session) {
                return Err(error(
                    ErrorCode::InvalidSession,
                    "Session has not completed its first interaction",
                    false,
                ));
            }
            state.sessions[index].title_generation_started = true;
            let session = state.sessions[index].clone();
            let event = pending(
                "session.title_generation_started",
                Some(session_id.to_owned()),
                &session,
            );
            let value = serde_json::to_value(&state).map_err(serialization_error)?;
            match self
                .store
                .commit(snapshot.revision, value, vec![event])
                .await
            {
                Ok(_) => return Ok((session, project.workdir.clone(), true)),
                Err(ControlStoreError::Conflict) => {}
                Err(error) => return Err(store_error(error)),
            }
        }
        Err(error(
            ErrorCode::RunQueueConflict,
            "concurrent Session title update did not settle",
            true,
        ))
    }

    async fn finish_title_generation(
        &self,
        session_id: &str,
        title: String,
        description: String,
    ) -> Result<SessionView, ApiError> {
        for _ in 0..4 {
            let snapshot = self.store.load().await.map_err(store_error)?;
            let mut state = decode_state(snapshot.value)?;
            let session = state
                .sessions
                .iter_mut()
                .find(|session| session.id == session_id)
                .ok_or_else(|| error(ErrorCode::SessionNotFound, "session not found", false))?;
            session.title = Some(title.clone());
            session.description.clone_from(&description);
            let session = session.clone();
            let event = pending(
                "session.title_generated",
                Some(session_id.to_owned()),
                &session,
            );
            let value = serde_json::to_value(&state).map_err(serialization_error)?;
            match self
                .store
                .commit(snapshot.revision, value, vec![event])
                .await
            {
                Ok(_) => return Ok(session),
                Err(ControlStoreError::Conflict) => {}
                Err(error) => return Err(store_error(error)),
            }
        }
        Err(error(
            ErrorCode::RunQueueConflict,
            "concurrent Session title update did not settle",
            true,
        ))
    }

    async fn execute_workspace_agent(&self, run: RunView) -> Result<RunView, ApiError> {
        let snapshot = self.store.load().await.map_err(store_error)?;
        let state = decode_state(snapshot.value)?;
        let agent = require_agent(&state, &run.agent_id)?.clone();
        if agent.mode != AgentMode::Codex {
            return Ok(run);
        }
        let project = state
            .projects
            .iter()
            .find(|project| project.id == run.project_id)
            .ok_or_else(|| error(ErrorCode::InvalidProject, "project not found", false))?;
        let user_text = state
            .messages
            .iter()
            .find(|message| message.id == run.base_message_id)
            .and_then(|message| message.text.clone())
            .ok_or_else(|| error(ErrorCode::MessageNotFound, "run input not found", false))?;
        let prompt = codex_prompt(&state, &run.base_message_id)?;
        let workdir = Path::new(&project.workdir).to_path_buf();
        if !self.set_run_running(&run.id).await? {
            let snapshot = self.store.load().await.map_err(store_error)?;
            let state = decode_state(snapshot.value)?;
            return state
                .runs
                .into_iter()
                .find(|candidate| candidate.id == run.id)
                .ok_or_else(|| error(ErrorCode::InvalidRun, "run not found", false));
        }
        let result = match &self.workspace_agent {
            Some(executor) => {
                executor
                    .invoke(WorkspaceAgentInvocation {
                        request_id: run.id.clone(),
                        model: agent.model,
                        reasoning_effort: run
                            .reasoning_effort
                            .map(|effort| effort.as_str().to_owned()),
                        prompt,
                        commit_subject: user_text,
                        cwd: workdir,
                        cancellation: tokio_util::sync::CancellationToken::new(),
                    })
                    .await
            }
            None => Err(DomainError::invariant(
                ErrorCode::InvalidConfiguration,
                "Codex workspace executor is not configured",
            )),
        };
        self.finish_workspace_run(&run.id, result).await
    }

    async fn set_run_running(&self, run_id: &str) -> Result<bool, ApiError> {
        for _ in 0..4 {
            let snapshot = self.store.load().await.map_err(store_error)?;
            let mut state = decode_state(snapshot.value)?;
            let run = state
                .runs
                .iter_mut()
                .find(|run| run.id == run_id)
                .ok_or_else(|| error(ErrorCode::InvalidRun, "run not found", false))?;
            if run.status != "queued" {
                return Ok(false);
            }
            run.status = "running".into();
            let event = pending("run.updated", Some(run_id.to_owned()), run);
            let value = serde_json::to_value(&state).map_err(serialization_error)?;
            match self
                .store
                .commit(snapshot.revision, value, vec![event])
                .await
            {
                Ok(_) => return Ok(true),
                Err(ControlStoreError::Conflict) => {}
                Err(error) => return Err(store_error(error)),
            }
        }
        Err(error(
            ErrorCode::RunQueueConflict,
            "concurrent run update did not settle",
            true,
        ))
    }

    async fn finish_workspace_run(
        &self,
        run_id: &str,
        result: Result<WorkspaceAgentResponse, DomainError>,
    ) -> Result<RunView, ApiError> {
        for _ in 0..4 {
            let snapshot = self.store.load().await.map_err(store_error)?;
            let mut state = decode_state(snapshot.value)?;
            let index = state
                .runs
                .iter()
                .position(|run| run.id == run_id)
                .ok_or_else(|| error(ErrorCode::InvalidRun, "run not found", false))?;
            let mut run = state.runs[index].clone();
            if matches!(run.status.as_str(), "completed" | "failed" | "cancelled") {
                return Ok(run);
            }
            match &result {
                Ok(output) => {
                    let data = output
                        .commit_id
                        .as_ref()
                        .map(|commit_id| json!({"codex":{"commit_id":commit_id}}));
                    let parent = run
                        .last_message_id
                        .as_deref()
                        .unwrap_or(&run.base_message_id)
                        .to_owned();
                    let reply = message(
                        &run.project_id,
                        Some(&parent),
                        "assistant",
                        "standard",
                        Some(output.assistant_text.clone()),
                        None,
                        data,
                    );
                    append_output(&mut state, &mut run, reply);
                    run.status = "completed".into();
                    run.error = None;
                }
                Err(failure) => {
                    run.status = "failed".into();
                    run.error = Some(error(failure.code, &failure.message, failure.retryable));
                }
            }
            release_session(&mut state, &run);
            state.runs[index] = run.clone();
            let event = pending("run.updated", Some(run_id.to_owned()), &run);
            let value = serde_json::to_value(&state).map_err(serialization_error)?;
            match self
                .store
                .commit(snapshot.revision, value, vec![event])
                .await
            {
                Ok(_) => return Ok(run),
                Err(ControlStoreError::Conflict) => {}
                Err(error) => return Err(store_error(error)),
            }
        }
        Err(error(
            ErrorCode::RunQueueConflict,
            "concurrent Codex completion did not settle",
            true,
        ))
    }

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

    async fn try_execute(&self, command: Command) -> Result<CommandResult, ApiError> {
        if matches!(
            command,
            Command::Snapshot
                | Command::GetRun { .. }
                | Command::ExportProject { .. }
                | Command::GetSettings
        ) {
            let snapshot = self.store.load().await.map_err(store_error)?;
            let state = decode_state(snapshot.value)?;
            return read_command(state, snapshot.revision, command);
        }

        for _ in 0..4 {
            let snapshot = self.store.load().await.map_err(store_error)?;
            let mut state = decode_state(snapshot.value)?;
            let git_commit = user_message_git_commit(&state, &command)?;
            let (result, events) =
                apply_command(&mut state, command.clone(), git_commit.as_deref())?;
            let value = serde_json::to_value(&state).map_err(serialization_error)?;
            match self.store.commit(snapshot.revision, value, events).await {
                Ok(_) => return Ok(result),
                Err(ControlStoreError::Conflict) => {}
                Err(error) => return Err(store_error(error)),
            }
        }
        Err(error(
            ErrorCode::RunQueueConflict,
            "concurrent state update did not settle",
            true,
        ))
    }
}

fn read_command(state: State, revision: u64, command: Command) -> Result<CommandResult, ApiError> {
    match command {
        Command::Snapshot => Ok(CommandResult::Workspace(state.into())),
        Command::GetRun { run_id } => state
            .runs
            .into_iter()
            .find(|run| run.id == run_id)
            .map(CommandResult::Run)
            .ok_or_else(|| error(ErrorCode::InvalidRun, "run not found", false)),
        Command::ExportProject { project_id } => {
            export_project(&state, revision, &project_id).map(CommandResult::ProjectExport)
        }
        Command::GetSettings => Ok(CommandResult::Settings(settings_view(&state))),
        _ => unreachable!("mutating command routed to read path"),
    }
}

fn apply_command(
    state: &mut State,
    command: Command,
    user_git_commit: Option<&str>,
) -> Result<(CommandResult, Vec<PendingEvent>), ApiError> {
    match command {
        Command::RegisterProject {
            id,
            name,
            workdir,
            repo_url,
        } => register_project(state, id, name, &workdir, repo_url),
        Command::SetProjectDefaultAgent {
            project_id,
            agent_id,
        } => set_project_default_agent(state, &project_id, &agent_id),
        Command::RegisterAgent {
            id,
            name,
            model,
            mode,
        } => register_agent(state, id, name, model, mode),
        Command::CreateSession {
            id,
            project_id,
            agent_id,
            at_message_id,
        } => create_session(state, id, project_id, agent_id, at_message_id),
        Command::SetSessionAgent {
            session_id,
            agent_id,
            expected_version,
        } => set_session_agent(state, &session_id, &agent_id, expected_version),
        Command::RenameSession { session_id, name } => rename_session(state, &session_id, &name),
        Command::SetSessionTitle { session_id, title } => {
            set_session_title(state, &session_id, &title)
        }
        Command::SendMessage {
            session_id,
            text,
            expected_version,
            reasoning_effort,
        } => send_message(
            state,
            session_id,
            text,
            expected_version,
            reasoning_effort,
            require_user_git_commit(user_git_commit)?,
        ),
        Command::ForkSession {
            id,
            project_id,
            agent_id,
            at_message_id,
            text,
            reasoning_effort,
        } => fork_session(
            state,
            ForkSessionInput {
                id,
                project_id,
                agent_id,
                at_message_id,
                text,
                reasoning_effort,
            },
            require_user_git_commit(user_git_commit)?,
        ),
        Command::CancelRun { run_id } => cancel_run(state, &run_id),
        Command::CreateCron {
            id,
            name,
            project_id,
            base_message_id,
            agent_id,
            schedule,
            timezone,
        } => create_cron(
            state,
            id,
            name,
            project_id,
            base_message_id,
            agent_id,
            schedule,
            timezone,
        ),
        Command::SetCronEnabled { cron_id, enabled } => set_cron_enabled(state, &cron_id, enabled),
        Command::TriggerCron {
            cron_id,
            scheduled_at,
        } => trigger_cron(state, &cron_id, scheduled_at),
        Command::ImportProject { archive, workdir } => import_project(state, archive, &workdir),
        Command::SaveSettings {
            expected_revision,
            values,
        } => save_settings(state, expected_revision, values),
        Command::ResetSettings => Ok(reset_settings(state)),
        Command::Snapshot
        | Command::GetRun { .. }
        | Command::ExportProject { .. }
        | Command::GetSettings => unreachable!("read command routed to write path"),
    }
}

fn set_project_default_agent(
    state: &mut State,
    project_id: &str,
    agent_id: &str,
) -> Result<(CommandResult, Vec<PendingEvent>), ApiError> {
    require_agent(state, agent_id)?;
    let project = state
        .projects
        .iter_mut()
        .find(|project| project.id == project_id)
        .ok_or_else(|| error(ErrorCode::InvalidProject, "project not found", false))?;
    project.default_agent_id = Some(agent_id.to_owned());
    project.revision = project.revision.saturating_add(1);
    let project = project.clone();
    Ok((
        CommandResult::Project(project.clone()),
        vec![pending(
            "project.default_agent_updated",
            Some(project_id.to_owned()),
            &project,
        )],
    ))
}

fn fork_session(
    state: &mut State,
    input: ForkSessionInput,
    git_commit: &str,
) -> Result<(CommandResult, Vec<PendingEvent>), ApiError> {
    let (_, mut events) = create_session(
        state,
        input.id.clone(),
        input.project_id,
        input.agent_id,
        Some(input.at_message_id),
    )?;
    let (result, mut run_events) = send_message(
        state,
        input.id,
        input.text,
        Some(1),
        input.reasoning_effort,
        git_commit,
    )?;
    events.append(&mut run_events);
    Ok((result, events))
}

fn require_user_git_commit(commit: Option<&str>) -> Result<&str, ApiError> {
    commit.ok_or_else(|| {
        error(
            ErrorCode::ProjectGitHeadUnavailable,
            "Git HEAD snapshot is missing for user message",
            false,
        )
    })
}

fn settings_view(state: &State) -> SettingsView {
    SettingsView {
        schema: settings_schema(),
        values: state.settings.clone(),
        revision: state.settings_revision,
    }
}

fn save_settings(
    state: &mut State,
    expected_revision: u64,
    values: SettingsDocument,
) -> Result<(CommandResult, Vec<PendingEvent>), ApiError> {
    if state.settings_revision != expected_revision {
        return Err(error(
            ErrorCode::InvalidConfiguration,
            "settings changed in another client; reload and try again",
            false,
        ));
    }
    validate_settings(&values)?;
    state.settings = values;
    state.settings_revision = state.settings_revision.saturating_add(1);
    let view = settings_view(state);
    Ok((
        CommandResult::Settings(view.clone()),
        vec![pending("settings.updated", None, &view)],
    ))
}

fn reset_settings(state: &mut State) -> (CommandResult, Vec<PendingEvent>) {
    state.settings = default_settings();
    state.settings_revision = state.settings_revision.saturating_add(1);
    let view = settings_view(state);
    (
        CommandResult::Settings(view.clone()),
        vec![pending("settings.reset", None, &view)],
    )
}

fn validate_settings(values: &SettingsDocument) -> Result<(), ApiError> {
    let schema = settings_schema();
    let expected = schema
        .definitions
        .iter()
        .map(|definition| definition.id.as_str())
        .collect::<HashSet<_>>();
    if values.0.keys().any(|key| !expected.contains(key.as_str())) {
        return Err(error(
            ErrorCode::InvalidConfiguration,
            "settings contain an unknown key",
            false,
        ));
    }
    for definition in schema.definitions {
        let Some(value) = values.0.get(&definition.id) else {
            return Err(error(
                ErrorCode::InvalidConfiguration,
                format!("missing setting {}", definition.id),
                false,
            ));
        };
        let valid = match &definition.kind {
            SettingKind::Text | SettingKind::Path | SettingKind::CredentialReference => {
                value.is_string()
            }
            SettingKind::Boolean => value.is_boolean(),
            SettingKind::Number { min, max } => value
                .as_i64()
                .is_some_and(|number| number >= *min && number <= *max),
            SettingKind::Select { options } => value
                .as_str()
                .is_some_and(|choice| options.iter().any(|option| option == choice)),
        };
        if !valid {
            return Err(error(
                ErrorCode::InvalidConfiguration,
                format!("invalid value for setting {}", definition.id),
                false,
            ));
        }
    }
    Ok(())
}

fn register_project(
    state: &mut State,
    id: String,
    name: String,
    workdir: &str,
    mut repo_url: Option<String>,
) -> Result<(CommandResult, Vec<PendingEvent>), ApiError> {
    if id.trim().is_empty() || name.trim().is_empty() {
        return Err(error(
            ErrorCode::InvalidProject,
            "project id and name are required",
            false,
        ));
    }
    if state.projects.iter().any(|project| project.id == id) {
        return Err(error(
            ErrorCode::InvalidProject,
            "project id already exists",
            false,
        ));
    }
    if let Some(url) = &mut repo_url {
        *url = url.trim().to_owned();
        if url.is_empty() {
            return Err(error(
                ErrorCode::InvalidProject,
                "repository URL cannot be empty",
                false,
            ));
        }
    }
    let canonical = prepare_git_root(Path::new(&workdir))?;
    let base_commit = ensure_git_head(&canonical)?;
    let canonical_text = canonical.to_string_lossy().into_owned();
    if state
        .projects
        .iter()
        .any(|project| project.workdir == canonical_text)
    {
        return Err(error(
            ErrorCode::ProjectPathAlreadyRegistered,
            "project path is already registered",
            false,
        ));
    }
    let root_id = Uuid::new_v4().to_string();
    let project = ProjectView {
        id: id.clone(),
        name,
        workdir: canonical_text,
        root_message_id: root_id.clone(),
        repo_url,
        base_commit,
        default_agent_id: None,
        revision: 1,
    };
    state.messages.push(MessageView {
        id: root_id,
        project_id: id.clone(),
        parent_message_id: None,
        role: "system".into(),
        kind: "standard".into(),
        text: Some("AIT project instructions".into()),
        git_commit: None,
        data: None,
    });
    state.projects.push(project.clone());
    Ok((
        CommandResult::Project(project.clone()),
        vec![pending("project.registered", Some(id), &project)],
    ))
}

fn register_agent(
    state: &mut State,
    id: String,
    name: String,
    model: String,
    mode: AgentMode,
) -> Result<(CommandResult, Vec<PendingEvent>), ApiError> {
    if id.trim().is_empty() || name.trim().is_empty() || model.trim().is_empty() {
        return Err(error(
            ErrorCode::InvalidAgentConfiguration,
            "agent id, name, and model are required",
            false,
        ));
    }
    if state.agents.iter().any(|agent| agent.id == id) {
        return Err(error(
            ErrorCode::InvalidAgentConfiguration,
            "agent id already exists",
            false,
        ));
    }
    let agent = AgentView {
        id: id.clone(),
        name,
        model,
        mode,
        revision: 1,
        enabled: true,
    };
    state.agents.push(agent.clone());
    Ok((
        CommandResult::Agent(agent.clone()),
        vec![pending("agent.registered", Some(id), &agent)],
    ))
}

fn create_session(
    state: &mut State,
    id: String,
    project_id: String,
    agent_id: String,
    at_message_id: Option<String>,
) -> Result<(CommandResult, Vec<PendingEvent>), ApiError> {
    if id.trim().is_empty() || state.sessions.iter().any(|session| session.id == id) {
        return Err(error(
            ErrorCode::InvalidSession,
            "session id is empty or already exists",
            false,
        ));
    }
    let project = state
        .projects
        .iter()
        .find(|project| project.id == project_id)
        .ok_or_else(|| error(ErrorCode::InvalidProject, "project not found", false))?;
    require_agent(state, &agent_id)?;
    let head = at_message_id.unwrap_or_else(|| project.root_message_id.clone());
    let target = state
        .messages
        .iter()
        .find(|message| message.id == head)
        .ok_or_else(|| {
            error(
                ErrorCode::MessageNotFound,
                "branch message not found",
                false,
            )
        })?;
    if target.project_id != project_id {
        return Err(error(
            ErrorCode::SessionMessageProjectMismatch,
            "branch message belongs to another project",
            false,
        ));
    }
    let session = SessionView {
        id: id.clone(),
        project_id,
        name: String::new(),
        title: None,
        description: String::new(),
        title_generation_started: false,
        agent_id,
        current_message_id: head,
        active_run_id: None,
        version: 1,
    };
    state.sessions.push(session.clone());
    Ok((
        CommandResult::Session(session.clone()),
        vec![pending("session.created", Some(id), &session)],
    ))
}

fn rename_session(
    state: &mut State,
    session_id: &str,
    name: &str,
) -> Result<(CommandResult, Vec<PendingEvent>), ApiError> {
    let name = name.split_whitespace().collect::<Vec<_>>().join(" ");
    if name.chars().count() > 100 {
        return Err(error(
            ErrorCode::InvalidSession,
            "Session name must be at most 100 characters",
            false,
        ));
    }
    let session = state
        .sessions
        .iter_mut()
        .find(|session| session.id == session_id)
        .ok_or_else(|| error(ErrorCode::SessionNotFound, "session not found", false))?;
    session.name = name;
    let session = session.clone();
    Ok((
        CommandResult::Session(session.clone()),
        vec![pending(
            "session.renamed",
            Some(session_id.to_owned()),
            &session,
        )],
    ))
}

fn set_session_title(
    state: &mut State,
    session_id: &str,
    title: &str,
) -> Result<(CommandResult, Vec<PendingEvent>), ApiError> {
    let title = title.split_whitespace().collect::<Vec<_>>().join(" ");
    if title.is_empty() || title.chars().count() > 60 {
        return Err(error(
            ErrorCode::InvalidSession,
            "Temporary Session title must contain 1 to 60 characters",
            false,
        ));
    }
    let session = state
        .sessions
        .iter_mut()
        .find(|session| session.id == session_id)
        .ok_or_else(|| error(ErrorCode::SessionNotFound, "session not found", false))?;
    if !session.title_generation_started {
        session.title = Some(title);
    }
    let session = session.clone();
    Ok((
        CommandResult::Session(session.clone()),
        vec![pending(
            "session.title_updated",
            Some(session_id.to_owned()),
            &session,
        )],
    ))
}

fn is_first_completed_interaction(state: &State, session: &SessionView) -> bool {
    let head_is_assistant = state
        .messages
        .iter()
        .find(|message| message.id == session.current_message_id)
        .is_some_and(|message| message.role == "assistant");
    head_is_assistant
        && state
            .runs
            .iter()
            .filter(|run| run.session_id.as_deref() == Some(session.id.as_str()))
            .count()
            == 1
}

fn validate_session_metadata(title: &str, description: &str) -> Result<(), ApiError> {
    let title = title.trim();
    let invalid_markup = [
        '"', '\'', '“', '”', '‘', '’', '#', '*', '`', '[', ']', '<', '>',
    ];
    let ending_punctuation = [
        '.', '。', '!', '！', '?', '？', ',', '，', ';', '；', ':', '：',
    ];
    if title.is_empty()
        || title.chars().count() > 36
        || title
            .chars()
            .any(|character| invalid_markup.contains(&character))
        || title
            .chars()
            .last()
            .is_some_and(|character| ending_punctuation.contains(&character))
        || description.trim().is_empty()
    {
        return Err(error(
            ErrorCode::InvalidSession,
            "generated Session metadata is outside the requested constraints",
            false,
        ));
    }
    Ok(())
}

fn set_session_agent(
    state: &mut State,
    session_id: &str,
    agent_id: &str,
    expected_version: Option<u64>,
) -> Result<(CommandResult, Vec<PendingEvent>), ApiError> {
    require_agent(state, agent_id)?;
    let session = state
        .sessions
        .iter_mut()
        .find(|session| session.id == session_id)
        .ok_or_else(|| error(ErrorCode::SessionNotFound, "session not found", false))?;
    if expected_version.is_some_and(|version| version != session.version) {
        return Err(error(
            ErrorCode::SessionPointerConflict,
            "session version changed",
            false,
        ));
    }
    if session.active_run_id.is_some() {
        return Err(error(
            ErrorCode::SessionBusy,
            "session already has an active run",
            false,
        ));
    }
    if session.agent_id == agent_id {
        return Ok((CommandResult::Session(session.clone()), Vec::new()));
    }
    agent_id.clone_into(&mut session.agent_id);
    session.version = session.version.saturating_add(1);
    let session = session.clone();
    Ok((
        CommandResult::Session(session.clone()),
        vec![pending(
            "session.agent_updated",
            Some(session_id.to_owned()),
            &session,
        )],
    ))
}

fn send_message(
    state: &mut State,
    session_id: String,
    text: String,
    expected_version: Option<u64>,
    reasoning_effort: Option<ReasoningEffort>,
    git_commit: &str,
) -> Result<(CommandResult, Vec<PendingEvent>), ApiError> {
    if text.trim().is_empty() {
        return Err(error(
            ErrorCode::InvalidMessageRole,
            "message text is required",
            false,
        ));
    }
    let index = state
        .sessions
        .iter()
        .position(|session| session.id == session_id)
        .ok_or_else(|| error(ErrorCode::SessionNotFound, "session not found", false))?;
    let session = state.sessions[index].clone();
    if expected_version.is_some_and(|version| version != session.version) {
        return Err(error(
            ErrorCode::SessionPointerConflict,
            "session version changed",
            false,
        ));
    }
    if session.active_run_id.is_some() {
        return Err(error(
            ErrorCode::SessionBusy,
            "session already has an active run",
            false,
        ));
    }
    let agent = require_agent(state, &session.agent_id)?.clone();
    validate_reasoning_effort(&agent, reasoning_effort)?;
    let user = message(
        &session.project_id,
        Some(&session.current_message_id),
        "user",
        "standard",
        Some(text),
        Some(git_commit),
        None,
    );
    state.messages.push(user.clone());
    let run_id = Uuid::new_v4().to_string();
    state.sessions[index]
        .current_message_id
        .clone_from(&user.id);
    state.sessions[index].version += 1;
    state.sessions[index].active_run_id = Some(run_id.clone());
    let run = RunView {
        id: run_id.clone(),
        project_id: session.project_id,
        base_message_id: user.id,
        last_message_id: None,
        session_id: Some(session_id),
        agent_id: agent.id.clone(),
        agent_revision: agent.revision,
        reasoning_effort,
        trigger: "manual".into(),
        cron_id: None,
        scheduled_at: None,
        status: "queued".into(),
        error: None,
    };
    state.runs.push(run);
    let run = execute_run(state, &run_id, &agent);
    Ok((
        CommandResult::Run(run.clone()),
        vec![pending("run.updated", Some(run_id), &run)],
    ))
}

fn validate_reasoning_effort(
    agent: &AgentView,
    reasoning_effort: Option<ReasoningEffort>,
) -> Result<(), ApiError> {
    if reasoning_effort.is_none() {
        return Ok(());
    }
    if agent.mode != AgentMode::Codex {
        return Err(error(
            ErrorCode::InvalidAgentConfiguration,
            "reasoning effort is only supported by Codex Agents",
            false,
        ));
    }
    if !matches!(agent.model.as_str(), "gpt-5.6-sol" | "gpt-5.6-codex") {
        return Err(error(
            ErrorCode::InvalidAgentConfiguration,
            "the selected Codex model does not advertise reasoning effort options",
            false,
        ));
    }
    Ok(())
}

fn execute_run(state: &mut State, run_id: &str, agent: &AgentView) -> RunView {
    let index = state
        .runs
        .iter()
        .position(|run| run.id == run_id)
        .expect("new run exists");
    let mut run = state.runs[index].clone();
    match agent.mode {
        AgentMode::Codex | AgentMode::Manual => {}
        AgentMode::ProviderFailure => {
            run.status = "failed".into();
            run.error = Some(error(
                ErrorCode::ProviderFailed,
                "provider invocation failed",
                true,
            ));
            release_session(state, &run);
        }
        AgentMode::ApprovalRequired => {
            run.status = "waiting_approval".into();
            run.error = Some(error(
                ErrorCode::ToolApprovalRequired,
                "tool execution requires approval",
                false,
            ));
        }
        AgentMode::Echo => {
            let reply = message(
                &run.project_id,
                Some(&run.base_message_id),
                "assistant",
                "standard",
                Some("echo: completed".into()),
                None,
                None,
            );
            append_output(state, &mut run, reply);
            run.status = "completed".into();
            release_session(state, &run);
        }
        AgentMode::Tool => {
            let tool = message(
                &run.project_id,
                Some(&run.base_message_id),
                "assistant",
                "standard",
                None,
                None,
                Some(
                    json!({"tool_use":{"call_id":"call-1","tool_name":"echo","arguments":{"text":"hello from tool"}}}),
                ),
            );
            append_output(state, &mut run, tool);
            let result = message(
                &run.project_id,
                run.last_message_id.as_deref(),
                "user",
                "tool_result",
                None,
                None,
                Some(
                    json!({"tool_result":{"call_id":"call-1","status":"succeeded","output":"hello from tool"}}),
                ),
            );
            append_output(state, &mut run, result);
            let reply = message(
                &run.project_id,
                run.last_message_id.as_deref(),
                "assistant",
                "standard",
                Some("tool call completed".into()),
                None,
                None,
            );
            append_output(state, &mut run, reply);
            run.status = "completed".into();
            release_session(state, &run);
        }
    }
    state.runs[index] = run.clone();
    run
}

fn codex_prompt(state: &State, head_id: &str) -> Result<String, ApiError> {
    let mut path = Vec::new();
    let mut current = Some(head_id);
    while let Some(id) = current {
        let message = state
            .messages
            .iter()
            .find(|message| message.id == id)
            .ok_or_else(|| {
                error(
                    ErrorCode::MessageNotFound,
                    "message path is incomplete",
                    false,
                )
            })?;
        path.push(message);
        current = message.parent_message_id.as_deref();
    }
    path.reverse();
    let mut prompt = String::from(
        "Work on the user's latest request in this repository. Make the requested code changes and verify them. Do not create a Git commit; the host will commit successful workspace changes.\n\nConversation:\n",
    );
    for message in path {
        if let Some(text) = &message.text {
            let _ = writeln!(prompt, "{}: {text}", message.role);
        }
    }
    Ok(prompt)
}

fn append_output(state: &mut State, run: &mut RunView, output: MessageView) {
    run.last_message_id = Some(output.id.clone());
    if let Some(session_id) = &run.session_id
        && let Some(session) = state
            .sessions
            .iter_mut()
            .find(|session| &session.id == session_id)
    {
        session.current_message_id.clone_from(&output.id);
        session.version += 1;
    }
    state.messages.push(output);
}

fn release_session(state: &mut State, run: &RunView) {
    if let Some(session_id) = &run.session_id
        && let Some(session) = state.sessions.iter_mut().find(|session| {
            &session.id == session_id && session.active_run_id.as_deref() == Some(run.id.as_str())
        })
    {
        session.active_run_id = None;
        session.version += 1;
    }
}

fn cancel_run(
    state: &mut State,
    run_id: &str,
) -> Result<(CommandResult, Vec<PendingEvent>), ApiError> {
    let index = state
        .runs
        .iter()
        .position(|run| run.id == run_id)
        .ok_or_else(|| error(ErrorCode::InvalidRun, "run not found", false))?;
    if matches!(
        state.runs[index].status.as_str(),
        "completed" | "failed" | "cancelled"
    ) {
        return Err(error(
            ErrorCode::RunAlreadyTerminal,
            "run is already terminal",
            false,
        ));
    }
    let mut run = state.runs[index].clone();
    run.status = "cancelled".into();
    run.error = Some(error(ErrorCode::RunCancelled, "run was cancelled", false));
    release_session(state, &run);
    state.runs[index] = run.clone();
    Ok((
        CommandResult::Run(run.clone()),
        vec![pending("run.cancelled", Some(run.id.clone()), &run)],
    ))
}

#[allow(clippy::too_many_arguments)]
fn create_cron(
    state: &mut State,
    id: String,
    name: String,
    project_id: String,
    base_message_id: String,
    agent_id: String,
    schedule: String,
    timezone: String,
) -> Result<(CommandResult, Vec<PendingEvent>), ApiError> {
    if state.crons.iter().any(|cron| cron.id == id) {
        return Err(error(
            ErrorCode::InvalidCron,
            "cron id already exists",
            false,
        ));
    }
    let base = state
        .messages
        .iter()
        .find(|message| message.id == base_message_id)
        .ok_or_else(|| {
            error(
                ErrorCode::CronBaseMessageUnavailable,
                "cron base message not found",
                false,
            )
        })?;
    if base.project_id != project_id {
        return Err(error(
            ErrorCode::CronBaseMessageUnavailable,
            "cron base message belongs to another project",
            false,
        ));
    }
    require_agent(state, &agent_id).map_err(|_| {
        error(
            ErrorCode::CronAgentUnavailable,
            "cron agent unavailable",
            false,
        )
    })?;
    let domain = Cron {
        id: CronId::new(&id),
        name: name.clone(),
        project_id: ProjectId::new(&project_id),
        base_message_id: MessageId::parse(&base_message_id)
            .map_err(|_| error(ErrorCode::InvalidCron, "invalid base message id", false))?,
        agent_id: AgentId::new(&agent_id),
        schedule: schedule.clone(),
        timezone: timezone.clone(),
        enabled: true,
        concurrency_policy: CronConcurrencyPolicy::Forbid,
        misfire_policy: CronMisfirePolicy::RunOnce,
        max_runtime: None,
        next_run_at: ait_scheduler::next_occurrence(&schedule, &timezone, TimestampMs(now()))
            .map_err(|failure| error(failure.code, failure.message, failure.retryable))?,
        last_run_at: None,
        version: 1,
        created_at: TimestampMs(now()),
        updated_at: TimestampMs(now()),
    };
    domain
        .validate()
        .map_err(|failure| error(failure.code, failure.message, failure.retryable))?;
    let cron = CronView {
        id: id.clone(),
        name,
        project_id,
        base_message_id,
        agent_id,
        schedule,
        timezone,
        enabled: true,
    };
    state.crons.push(cron.clone());
    Ok((
        CommandResult::Cron(cron.clone()),
        vec![pending("cron.created", Some(id), &cron)],
    ))
}

fn set_cron_enabled(
    state: &mut State,
    cron_id: &str,
    enabled: bool,
) -> Result<(CommandResult, Vec<PendingEvent>), ApiError> {
    let cron = state
        .crons
        .iter_mut()
        .find(|cron| cron.id == cron_id)
        .ok_or_else(|| error(ErrorCode::InvalidCron, "cron not found", false))?;
    cron.enabled = enabled;
    let cron = cron.clone();
    Ok((
        CommandResult::Cron(cron.clone()),
        vec![pending(
            "cron.enabled_changed",
            Some(cron.id.clone()),
            &cron,
        )],
    ))
}

fn trigger_cron(
    state: &mut State,
    cron_id: &str,
    scheduled_at: i64,
) -> Result<(CommandResult, Vec<PendingEvent>), ApiError> {
    if let Some(existing) = state.runs.iter().find(|run| {
        run.cron_id.as_deref() == Some(cron_id) && run.scheduled_at == Some(scheduled_at)
    }) {
        return Ok((CommandResult::Run(existing.clone()), Vec::new()));
    }
    let cron = state
        .crons
        .iter()
        .find(|cron| cron.id == cron_id && cron.enabled)
        .cloned()
        .ok_or_else(|| error(ErrorCode::InvalidCron, "enabled cron not found", false))?;
    let agent = require_agent(state, &cron.agent_id)?.clone();
    let run_id = Uuid::new_v4().to_string();
    state.runs.push(RunView {
        id: run_id.clone(),
        project_id: cron.project_id,
        base_message_id: cron.base_message_id,
        last_message_id: None,
        session_id: None,
        agent_id: agent.id.clone(),
        agent_revision: agent.revision,
        reasoning_effort: None,
        trigger: "cron".into(),
        cron_id: Some(cron.id),
        scheduled_at: Some(scheduled_at),
        status: "queued".into(),
        error: None,
    });
    let run = execute_run(state, &run_id, &agent);
    Ok((
        CommandResult::Run(run.clone()),
        vec![pending("cron.run_triggered", Some(run_id), &run)],
    ))
}

fn export_project(
    state: &State,
    source_revision: u64,
    project_id: &str,
) -> Result<ProjectExport, ApiError> {
    let project = state
        .projects
        .iter()
        .find(|project| project.id == project_id)
        .cloned()
        .ok_or_else(|| error(ErrorCode::InvalidProject, "project not found", false))?;
    let messages = state
        .messages
        .iter()
        .filter(|message| message.project_id == project_id)
        .cloned()
        .collect::<Vec<_>>();
    let sessions = state
        .sessions
        .iter()
        .filter(|session| session.project_id == project_id)
        .cloned()
        .map(|mut session| {
            // An active Run is process-local state and cannot safely be resumed
            // from a portable archive.
            session.active_run_id = None;
            session
        })
        .collect::<Vec<_>>();
    let mut referenced_agents = sessions
        .iter()
        .map(|session| session.agent_id.as_str())
        .collect::<HashSet<_>>();
    if let Some(default_agent_id) = project.default_agent_id.as_deref() {
        referenced_agents.insert(default_agent_id);
    }
    let agents = state
        .agents
        .iter()
        .filter(|agent| referenced_agents.contains(agent.id.as_str()))
        .cloned()
        .collect();
    let archive = ProjectExport {
        format_version: PROJECT_EXPORT_VERSION,
        source_revision,
        project,
        agents,
        sessions,
        messages,
    };
    validate_project_export(&archive)?;
    Ok(archive)
}

fn import_project(
    state: &mut State,
    archive: ProjectExport,
    workdir: &str,
) -> Result<(CommandResult, Vec<PendingEvent>), ApiError> {
    validate_project_export(&archive)?;
    if state
        .projects
        .iter()
        .any(|project| project.id == archive.project.id)
    {
        return Err(error(
            ErrorCode::InvalidProject,
            "project id already exists",
            false,
        ));
    }
    if archive.messages.iter().any(|imported| {
        state
            .messages
            .iter()
            .any(|existing| existing.id == imported.id)
    }) || archive.sessions.iter().any(|imported| {
        state
            .sessions
            .iter()
            .any(|existing| existing.id == imported.id)
    }) {
        return Err(error(
            ErrorCode::InvalidProject,
            "archive identity conflicts with existing workspace state",
            false,
        ));
    }
    for imported in &archive.agents {
        if let Some(existing) = state
            .agents
            .iter()
            .find(|existing| existing.id == imported.id)
            && existing != imported
        {
            return Err(error(
                ErrorCode::InvalidAgentConfiguration,
                "archive agent conflicts with an existing revision",
                false,
            ));
        }
    }

    let canonical = prepare_git_root(Path::new(workdir))?;
    let canonical_text = canonical.to_string_lossy().into_owned();
    if state
        .projects
        .iter()
        .any(|project| project.workdir == canonical_text)
    {
        return Err(error(
            ErrorCode::ProjectPathAlreadyRegistered,
            "project path is already registered",
            false,
        ));
    }

    let mut project = archive.project;
    project.workdir = canonical_text;
    project.base_commit = ensure_git_head(&canonical)?;
    for agent in archive.agents {
        if !state.agents.iter().any(|existing| existing.id == agent.id) {
            state.agents.push(agent);
        }
    }
    state.messages.extend(archive.messages);
    state.sessions.extend(archive.sessions);
    state.projects.push(project.clone());
    Ok((
        CommandResult::Project(project.clone()),
        vec![pending(
            "project.imported",
            Some(project.id.clone()),
            &project,
        )],
    ))
}

fn validate_project_export(archive: &ProjectExport) -> Result<(), ApiError> {
    if archive.format_version != PROJECT_EXPORT_VERSION
        || archive.source_revision == 0
        || archive.project.id.trim().is_empty()
        || archive.project.revision == 0
        || !is_git_commit(&archive.project.base_commit)
        || archive
            .project
            .repo_url
            .as_ref()
            .is_some_and(|url| url.trim().is_empty())
        || archive.messages.is_empty()
    {
        return Err(invalid_archive(
            "archive header or project revision is invalid",
        ));
    }

    let mut message_by_id = HashMap::with_capacity(archive.messages.len());
    for message in &archive.messages {
        if message.project_id != archive.project.id
            || Uuid::parse_str(&message.id).is_err()
            || (message.role == "user"
                && message.kind == "standard"
                && message
                    .git_commit
                    .as_deref()
                    .is_none_or(|commit| !is_git_commit(commit)))
            || (message.git_commit.is_some()
                && (message.role != "user" || message.kind != "standard"))
            || message_by_id.insert(message.id.as_str(), message).is_some()
        {
            return Err(invalid_archive(
                "archive message identity or project ownership is invalid",
            ));
        }
    }
    let Some(root) = message_by_id.get(archive.project.root_message_id.as_str()) else {
        return Err(invalid_archive("archive root message is missing"));
    };
    if root.parent_message_id.is_some() || root.role != "system" {
        return Err(invalid_archive("archive root message is invalid"));
    }
    for message in &archive.messages {
        let mut cursor = message;
        let mut seen = HashSet::new();
        while cursor.id != archive.project.root_message_id {
            if !seen.insert(cursor.id.as_str()) {
                return Err(invalid_archive("archive message graph contains a cycle"));
            }
            let Some(parent_id) = cursor.parent_message_id.as_deref() else {
                return Err(invalid_archive(
                    "archive message graph contains an unexpected root",
                ));
            };
            cursor = message_by_id
                .get(parent_id)
                .copied()
                .ok_or_else(|| invalid_archive("archive message parent is missing"))?;
        }
    }

    let mut agent_ids = HashSet::with_capacity(archive.agents.len());
    if archive.agents.iter().any(|agent| {
        agent.id.trim().is_empty() || agent.revision == 0 || !agent_ids.insert(agent.id.as_str())
    }) {
        return Err(invalid_archive("archive agent revision is invalid"));
    }
    if archive
        .project
        .default_agent_id
        .as_deref()
        .is_some_and(|agent_id| !agent_ids.contains(agent_id))
    {
        return Err(invalid_archive(
            "archive Project default Agent binding is invalid",
        ));
    }
    let mut session_ids = HashSet::with_capacity(archive.sessions.len());
    for session in &archive.sessions {
        if session.project_id != archive.project.id
            || session.version == 0
            || session.active_run_id.is_some()
            || !session_ids.insert(session.id.as_str())
            || !message_by_id.contains_key(session.current_message_id.as_str())
            || !agent_ids.contains(session.agent_id.as_str())
        {
            return Err(invalid_archive(
                "archive session pointer, agent binding, or revision is invalid",
            ));
        }
    }
    Ok(())
}

fn invalid_archive(message: impl Into<String>) -> ApiError {
    error(ErrorCode::InvalidProject, message, false)
}

fn require_agent<'a>(state: &'a State, id: &str) -> Result<&'a AgentView, ApiError> {
    state
        .agents
        .iter()
        .find(|agent| agent.id == id && agent.enabled)
        .ok_or_else(|| error(ErrorCode::AgentNotFound, "enabled agent not found", false))
}

fn message(
    project: &str,
    parent: Option<&str>,
    role: &str,
    kind: &str,
    text: Option<String>,
    git_commit: Option<&str>,
    data: Option<Value>,
) -> MessageView {
    MessageView {
        id: Uuid::new_v4().to_string(),
        project_id: project.into(),
        parent_message_id: parent.map(str::to_owned),
        role: role.into(),
        kind: kind.into(),
        text,
        git_commit: git_commit.map(str::to_owned),
        data,
    }
}

fn prepare_git_root(path: &Path) -> Result<std::path::PathBuf, ApiError> {
    if !path.exists() {
        return Err(error(
            ErrorCode::ProjectPathNotFound,
            "project path does not exist",
            false,
        ));
    }
    if !path.is_dir() {
        return Err(error(
            ErrorCode::ProjectPathNotDirectory,
            "project path is not a directory",
            false,
        ));
    }
    let canonical = path
        .canonicalize()
        .map_err(|failure| error(ErrorCode::ProjectPathNotFound, failure.to_string(), false))?;
    let top = git_top_level(&canonical);
    if top.as_deref() != Some(canonical.as_path()) {
        let output = ProcessCommand::new("git")
            .arg("-C")
            .arg(&canonical)
            .arg("init")
            .output()
            .map_err(|failure| {
                error(ErrorCode::ProjectGitInitFailed, failure.to_string(), false)
            })?;
        if !output.status.success() {
            return Err(error(
                ErrorCode::ProjectGitInitFailed,
                String::from_utf8_lossy(&output.stderr).into_owned(),
                false,
            ));
        }
    }
    if git_top_level(&canonical).as_deref() != Some(canonical.as_path()) {
        return Err(error(
            ErrorCode::ProjectGitInitFailed,
            "git root verification failed",
            false,
        ));
    }
    Ok(canonical)
}

fn ensure_git_head(path: &Path) -> Result<String, ApiError> {
    if let Some(head) = git_head(path)? {
        return Ok(head);
    }
    let staged = ProcessCommand::new("git")
        .arg("-C")
        .arg(path)
        .args(["diff", "--cached", "--quiet", "--exit-code"])
        .output()
        .map_err(|failure| {
            error(
                ErrorCode::ProjectGitHeadUnavailable,
                failure.to_string(),
                false,
            )
        })?;
    if !staged.status.success() {
        return Err(error(
            ErrorCode::ProjectGitHeadUnavailable,
            "cannot create an empty initial commit while the index contains staged changes",
            false,
        ));
    }
    let output = ProcessCommand::new("git")
        .arg("-C")
        .arg(path)
        .args([
            "-c",
            "user.name=AIT",
            "-c",
            "user.email=ait@localhost",
            "commit",
            "--allow-empty",
            "--no-gpg-sign",
            "--no-verify",
            "--quiet",
            "-m",
            "Initialize AIT project",
        ])
        .output()
        .map_err(|failure| {
            error(
                ErrorCode::ProjectGitHeadUnavailable,
                failure.to_string(),
                false,
            )
        })?;
    if !output.status.success() {
        return Err(error(
            ErrorCode::ProjectGitHeadUnavailable,
            String::from_utf8_lossy(&output.stderr).trim(),
            false,
        ));
    }
    git_head(path)?.ok_or_else(|| {
        error(
            ErrorCode::ProjectGitHeadUnavailable,
            "initial commit succeeded but Git HEAD is unavailable",
            false,
        )
    })
}

fn user_message_git_commit(state: &State, command: &Command) -> Result<Option<String>, ApiError> {
    let project_id = match command {
        Command::SendMessage { session_id, .. } => Some(
            state
                .sessions
                .iter()
                .find(|session| session.id == *session_id)
                .ok_or_else(|| error(ErrorCode::SessionNotFound, "session not found", false))?
                .project_id
                .as_str(),
        ),
        Command::ForkSession { project_id, .. } => Some(project_id.as_str()),
        _ => return Ok(None),
    };
    let Some(project_id) = project_id else {
        return Ok(None);
    };
    let project = state
        .projects
        .iter()
        .find(|project| project.id == project_id)
        .ok_or_else(|| error(ErrorCode::InvalidProject, "project not found", false))?;
    clean_git_head(Path::new(&project.workdir)).map(Some)
}

fn clean_git_head(path: &Path) -> Result<String, ApiError> {
    let before = git_head(path)?.ok_or_else(|| {
        error(
            ErrorCode::ProjectGitHeadUnavailable,
            "project repository has no HEAD commit",
            false,
        )
    })?;
    let status = ProcessCommand::new("git")
        .arg("-C")
        .arg(path)
        .args(["status", "--porcelain=v1", "--untracked-files=normal"])
        .output()
        .map_err(|failure| {
            error(
                ErrorCode::ProjectGitHeadUnavailable,
                failure.to_string(),
                false,
            )
        })?;
    if !status.status.success() {
        return Err(error(
            ErrorCode::ProjectGitHeadUnavailable,
            String::from_utf8_lossy(&status.stderr).trim(),
            false,
        ));
    }
    if !status.stdout.is_empty() {
        return Err(error(
            ErrorCode::ProjectGitDirty,
            "project Git worktree and index must be clean before adding a user message",
            false,
        ));
    }
    let after = git_head(path)?.ok_or_else(|| {
        error(
            ErrorCode::ProjectGitHeadUnavailable,
            "project repository HEAD disappeared while adding a user message",
            true,
        )
    })?;
    if before != after {
        return Err(error(
            ErrorCode::ProjectGitHeadUnavailable,
            "project repository HEAD changed while adding a user message; retry",
            true,
        ));
    }
    Ok(after)
}

fn git_head(path: &Path) -> Result<Option<String>, ApiError> {
    let output = ProcessCommand::new("git")
        .arg("-C")
        .arg(path)
        .args(["rev-parse", "--verify", "HEAD"])
        .output()
        .map_err(|failure| {
            error(
                ErrorCode::ProjectGitHeadUnavailable,
                failure.to_string(),
                false,
            )
        })?;
    if !output.status.success() {
        return Ok(None);
    }
    let head = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if is_git_commit(&head) {
        Ok(Some(head))
    } else {
        Err(error(
            ErrorCode::ProjectGitHeadUnavailable,
            "Git returned an invalid full HEAD object id",
            false,
        ))
    }
}

fn is_git_commit(value: &str) -> bool {
    matches!(value.len(), 40 | 64)
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn git_top_level(path: &Path) -> Option<std::path::PathBuf> {
    let output = ProcessCommand::new("git")
        .arg("-C")
        .arg(path)
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Path::new(String::from_utf8_lossy(&output.stdout).trim())
        .canonicalize()
        .ok()
}

fn pending<T: Serialize>(kind: &str, entity_id: Option<String>, body: &T) -> PendingEvent {
    PendingEvent {
        kind: kind.into(),
        entity_id,
        body: serde_json::to_value(body).unwrap_or(Value::Null),
        created_at: now(),
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
fn error(code: ErrorCode, message: impl Into<String>, retryable: bool) -> ApiError {
    ApiError {
        code,
        message: message.into(),
        retryable,
    }
}
#[allow(clippy::needless_pass_by_value)]
fn store_error(failure: ControlStoreError) -> ApiError {
    error(ErrorCode::RunRecoveryFailed, failure.to_string(), true)
}
#[allow(clippy::needless_pass_by_value)]
fn serialization_error(failure: serde_json::Error) -> ApiError {
    error(ErrorCode::RunRecoveryFailed, failure.to_string(), false)
}
fn decode_state(value: Value) -> Result<State, ApiError> {
    if value.is_null() {
        Ok(State::default())
    } else {
        serde_json::from_value(value).map_err(serialization_error)
    }
}
