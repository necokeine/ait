#![allow(missing_docs)]

use std::{
    collections::{HashMap, HashSet},
    fmt::Write as _,
    fs::{File, OpenOptions},
    panic::AssertUnwindSafe,
    path::{Path, PathBuf},
    process::Command as ProcessCommand,
    sync::{Arc, Mutex, Weak},
    time::{SystemTime, UNIX_EPOCH},
};

use ait_contracts::{
    API_VERSION, AgentConfiguration, AgentMode, AgentProvider, AgentProviderView, AgentView,
    ApiError, Command, CommandResult, CronView, Event, EventPage, MessageView,
    PROJECT_EXPORT_VERSION, ProjectExport, ProjectView, ProviderModel, Response, RunPartialOutput,
    RunView, RunWorktreeChange, RunWorktreeState, SessionView, SettingKind, SettingsDocument,
    SettingsView, WorkspaceView, default_settings, settings_schema,
};
use ait_domain::{
    AgentId, Cron, CronConcurrencyPolicy, CronId, CronMisfirePolicy, DomainError, ErrorCode,
    MessageId, ProjectId, RunId, RunTrigger, TimestampMs,
};
use ait_ports::{
    AdoptedWorkspace, AgentProviderGateway, ControlStore, ControlStoreError,
    HostProviderModelCatalog, PendingEvent, ProviderMessage, RunOutputArchive,
    SessionTitleGenerator, SessionTitleRequest, WorkspaceAgent, WorkspaceAgentInvocation,
    WorkspaceAgentResponse, WorkspaceIntegrationGate, WorkspaceOutputItem,
};
use futures_util::FutureExt;
use serde::{Deserialize, Serialize, Serializer};
use serde_json::{Value, json};
use uuid::Uuid;

mod agents;
mod progress;
use agents::{
    InvocationGuard, agent_for_session, builtin_providers, check_session_admission, migrate_state,
    register_agent, require_named_agent, set_session_config, update_agent, validate_config,
    validate_provider,
};
use progress::{FinishedProgress, ProgressPump};

const MAX_CONTROL_ID_BYTES: usize = 256;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct State {
    projects: Vec<ProjectView>,
    agents: Vec<AgentView>,
    #[serde(default = "builtin_providers")]
    providers: Vec<AgentProviderView>,
    #[serde(default)]
    provider_credentials: HashMap<String, String>,
    #[serde(default)]
    run_credentials: HashMap<String, String>,
    sessions: Vec<SessionView>,
    messages: Vec<MessageView>,
    #[serde(serialize_with = "serialize_runs_without_output")]
    runs: Vec<RunView>,
    crons: Vec<CronView>,
    #[serde(default = "default_settings")]
    settings: SettingsDocument,
    #[serde(default = "default_settings_revision")]
    settings_revision: u64,
}

fn serialize_runs_without_output<S>(runs: &[RunView], serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    let mut persisted = runs.to_vec();
    for run in &mut persisted {
        run.partial_output = None;
    }
    persisted.serialize(serializer)
}

struct ForkSessionInput {
    id: String,
    project_id: String,
    agent_id: String,
    at_message_id: String,
    text: String,
}

/// Internal continuation produced by a command, consumed only after its state
/// is committed. A public `RunView` is a result snapshot, never an execution signal.
enum CommandOutcome {
    Ready(Box<CommandResult>),
    ExecuteWorkspaceRun(Box<RunView>),
}

/// Owns both the async in-process queue position and the process-wide advisory
/// lock for one canonical Project Git worktree.
struct WorkspaceWriteLease {
    _process_guard: tokio::sync::OwnedMutexGuard<()>,
    _file: File,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct GitBaseline {
    commit: String,
    index_tree: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum WorkspaceFinalizationDecision {
    Open,
    Integrating,
    Cancelled,
}

#[derive(Debug)]
struct WorkspaceRunControl {
    cancellation: tokio_util::sync::CancellationToken,
    finalization: tokio::sync::Mutex<WorkspaceFinalizationDecision>,
}

impl WorkspaceRunControl {
    fn new() -> Self {
        Self {
            cancellation: tokio_util::sync::CancellationToken::new(),
            finalization: tokio::sync::Mutex::new(WorkspaceFinalizationDecision::Open),
        }
    }
}

#[async_trait::async_trait]
impl WorkspaceIntegrationGate for WorkspaceRunControl {
    async fn begin_integration(&self) -> Result<(), DomainError> {
        let mut decision = self.finalization.lock().await;
        match *decision {
            WorkspaceFinalizationDecision::Open if !self.cancellation.is_cancelled() => {
                *decision = WorkspaceFinalizationDecision::Integrating;
                Ok(())
            }
            WorkspaceFinalizationDecision::Integrating => Ok(()),
            WorkspaceFinalizationDecision::Open | WorkspaceFinalizationDecision::Cancelled => {
                Err(DomainError::invariant(
                    ErrorCode::RunCancelled,
                    "run cancellation won before workspace integration",
                ))
            }
        }
    }
}

struct WorkspaceRunControlGuard {
    controls: Arc<Mutex<HashMap<String, Weak<WorkspaceRunControl>>>>,
    id: String,
}

impl WorkspaceRunControlGuard {
    fn new(
        controls: Arc<Mutex<HashMap<String, Weak<WorkspaceRunControl>>>>,
        id: &str,
        control: &Arc<WorkspaceRunControl>,
    ) -> Self {
        controls
            .lock()
            .expect("workspace run controls")
            .insert(id.to_owned(), Arc::downgrade(control));
        Self {
            controls,
            id: id.to_owned(),
        }
    }
}

impl Drop for WorkspaceRunControlGuard {
    fn drop(&mut self) {
        self.controls
            .lock()
            .expect("workspace run controls")
            .remove(&self.id);
    }
}

impl CommandOutcome {
    fn for_new_run(run: RunView) -> Self {
        Self::ExecuteWorkspaceRun(Box::new(run))
    }
}

impl Default for State {
    fn default() -> Self {
        Self {
            projects: Vec::new(),
            agents: Vec::new(),
            providers: builtin_providers(),
            provider_credentials: HashMap::new(),
            run_credentials: HashMap::new(),
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
            providers: state.providers,
            sessions: state.sessions,
            messages: state.messages,
            runs: state.runs,
            crons: state.crons,
        }
    }
}

/// Shared application entry point used by every transport adapter.
#[derive(Clone)]
pub struct LocalControlService {
    store: Arc<dyn ControlStore>,
    session_leases: Arc<Mutex<HashMap<String, Weak<()>>>>,
    workspace_leases: Arc<Mutex<HashMap<PathBuf, Weak<tokio::sync::Mutex<()>>>>>,
    cancellations: Arc<Mutex<HashMap<String, tokio_util::sync::CancellationToken>>>,
    workspace_run_controls: Arc<Mutex<HashMap<String, Weak<WorkspaceRunControl>>>>,
    provider_gateway: Option<Arc<dyn AgentProviderGateway>>,
    host_provider_catalog: Option<Arc<dyn HostProviderModelCatalog>>,
    workspace_agent: Option<Arc<dyn WorkspaceAgent>>,
    session_title_generator: Option<Arc<dyn SessionTitleGenerator>>,
}

impl LocalControlService {
    #[must_use]
    pub fn new(store: Arc<dyn ControlStore>) -> Self {
        Self {
            store,
            session_leases: Arc::new(Mutex::new(HashMap::new())),
            workspace_leases: Arc::new(Mutex::new(HashMap::new())),
            cancellations: Arc::new(Mutex::new(HashMap::new())),
            workspace_run_controls: Arc::new(Mutex::new(HashMap::new())),
            provider_gateway: None,
            host_provider_catalog: None,
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
            session_leases: Arc::new(Mutex::new(HashMap::new())),
            workspace_leases: Arc::new(Mutex::new(HashMap::new())),
            cancellations: Arc::new(Mutex::new(HashMap::new())),
            workspace_run_controls: Arc::new(Mutex::new(HashMap::new())),
            provider_gateway: None,
            host_provider_catalog: None,
            workspace_agent: Some(workspace_agent),
            session_title_generator: None,
        }
    }

    #[must_use]
    pub fn with_provider_gateway(mut self, gateway: Arc<dyn AgentProviderGateway>) -> Self {
        self.provider_gateway = Some(gateway);
        self
    }

    /// Adds model discovery for host-authenticated providers such as Codex.
    #[must_use]
    pub fn with_host_provider_catalog(
        mut self,
        catalog: Arc<dyn HostProviderModelCatalog>,
    ) -> Self {
        self.host_provider_catalog = Some(catalog);
        self
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
            Ok(result) => Response::success(result),
            Err(error) => Response::failure(error),
        }
    }

    /// Persists a new interactive Run and returns immediately while the
    /// daemon-owned task continues independently of the requesting client.
    pub async fn submit(self: &Arc<Self>, command: Command) -> Response {
        match self.try_submit(command).await {
            Ok(result) => Response::success(result),
            Err(error) => Response::failure(error),
        }
    }

    async fn try_submit(self: &Arc<Self>, command: Command) -> Result<CommandResult, ApiError> {
        if !matches!(
            command,
            Command::SendMessage { .. } | Command::ForkSession { .. } | Command::ContinueRun { .. }
        ) {
            return Err(error(
                ErrorCode::InvalidConfiguration,
                "only interactive and recovery Run commands support asynchronous submission",
                false,
            ));
        }
        let session_lease = self.acquire_session(&command)?;
        let workspace_lease = self.acquire_workspace_write(&command).await?;
        let has_workspace_lease = workspace_lease.is_some();
        match self
            .commit_with_finalization_gate(command, has_workspace_lease)
            .await?
        {
            CommandOutcome::ExecuteWorkspaceRun(run) => {
                let accepted = (*run).clone();
                let run_id = run.id.clone();
                let control = Arc::new(WorkspaceRunControl::new());
                let invocation = InvocationGuard::new(
                    Arc::clone(&self.cancellations),
                    &run_id,
                    control.cancellation.clone(),
                );
                let control_guard = WorkspaceRunControlGuard::new(
                    Arc::clone(&self.workspace_run_controls),
                    &run_id,
                    &control,
                );
                let service = Arc::clone(self);
                tokio::spawn(async move {
                    let _owned = (session_lease, workspace_lease, invocation, control_guard);
                    let _ = service.supervise_workspace_agent(run_id, control).await;
                });
                Ok(CommandResult::Run(accepted))
            }
            CommandOutcome::Ready(_) => unreachable!("interactive submission creates a Run"),
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

    #[allow(
        clippy::too_many_lines,
        reason = "the invocation, cancellation race, progress drain, and terminal handoff are one barrier"
    )]
    async fn execute_workspace_agent(
        &self,
        run_id: &str,
        control: Arc<WorkspaceRunControl>,
    ) -> Result<RunView, ApiError> {
        let snapshot = self.store.load().await.map_err(store_error)?;
        let state = decode_state(snapshot.value)?;
        let run = state
            .runs
            .iter()
            .find(|run| run.id == run_id)
            .ok_or_else(|| error(ErrorCode::InvalidRun, "run not found", false))?;
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
        let message_baseline = state
            .messages
            .iter()
            .find(|message| message.id == run.base_message_id)
            .and_then(|message| message.git_commit.clone());
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
        let progress = ProgressPump::start(self.store.clone(), run);
        let reporter = progress.reporter();
        let cancellation = control.cancellation.clone();
        let call = async {
            match run.provider.kind {
                AgentMode::OpenAI | AgentMode::DeepSeek => self.invoke_provider(&state, run).await,
                AgentMode::Codex => {
                    self.invoke_codex_workspace(
                        &state,
                        run,
                        workdir.clone(),
                        user_text,
                        message_baseline,
                        control.clone(),
                        reporter.clone(),
                    )
                    .await
                }
            }
        };
        let result = AssertUnwindSafe(async {
            if run.provider.kind == AgentMode::Codex {
                // Workspace execution owns its complete settlement path. The
                // supervisor containing this call survives transport cancellation.
                call.await
            } else {
                tokio::pin!(call);
                tokio::select! {
                    biased;
                    () = cancellation.cancelled() => {
                        Err(DomainError::invariant(ErrorCode::RunCancelled, "run was cancelled"))
                    },
                    result = &mut call => result,
                }
            }
        })
        .catch_unwind()
        .await;
        drop(reporter);
        // Always join the writer and retain its final in-memory projection,
        // even when checkpoint persistence itself failed.
        let progress = progress.finish().await;
        let result = result.unwrap_or_else(|_| {
            Err(DomainError::invariant(
                ErrorCode::ProviderFailed,
                "workspace agent task panicked",
            ))
        });
        let result = if run.provider.kind == AgentMode::Codex {
            match result {
                Ok(output) => control.begin_integration().await.map(|()| output),
                Err(failure) => Err(failure),
            }
        } else {
            result
        };
        let terminal_output_needed = if result.is_ok() {
            // Settling is an informational state for the live UI. Once the
            // workspace result exists, failure to expose that intermediate
            // state must not bypass the reliable terminal persistence path.
            // `Ok(false)` means cancellation already won the terminal CAS, so
            // preserve the provider's complete final checkpoint on that Run.
            // A store error is retried by terminal persistence and is not
            // itself partial Agent output.
            self.set_run_settling(&run.id)
                .await
                .is_ok_and(|settled| !settled)
        } else {
            true
        };
        let partial_output = if terminal_output_needed {
            terminal_partial_output(progress, &workdir, result.as_ref().err())?
        } else {
            None
        };
        self.finish_workspace_run(&run.id, result, partial_output)
            .await
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "the invocation keeps immutable admission, finalization, and progress context together"
    )]
    async fn invoke_codex_workspace(
        &self,
        state: &State,
        run: &RunView,
        workdir: PathBuf,
        user_text: String,
        message_baseline: Option<String>,
        control: Arc<WorkspaceRunControl>,
        progress: Arc<dyn ait_ports::WorkspaceProgressReporter>,
    ) -> Result<WorkspaceAgentResponse, DomainError> {
        let executor = self.workspace_agent.as_ref().ok_or_else(|| {
            DomainError::invariant(
                ErrorCode::InvalidConfiguration,
                "Codex workspace executor is not configured",
            )
        })?;
        let baseline_commit = match run
            .workspace_base_commit
            .as_ref()
            .or(message_baseline.as_ref())
        {
            Some(commit) => commit.clone(),
            None => git_head(&workdir)
                .map_err(api_domain_error)?
                .ok_or_else(|| {
                    DomainError::invariant(
                        ErrorCode::ProjectGitHeadUnavailable,
                        "project repository has no HEAD commit",
                    )
                })?,
        };
        let baseline_index_tree = run
            .workspace_base_index_tree
            .as_deref()
            .map(str::to_owned)
            .ok_or_else(|| {
                DomainError::invariant(
                    ErrorCode::ProjectGitHeadUnavailable,
                    "Codex Run has no authorized Git index baseline",
                )
            })?;
        let adopted_worktree = self
            .recovery_workspace(run)
            .await
            .map_err(api_domain_error)?;
        let (mut project_instructions, prompt) =
            codex_prompt(state, &run.base_message_id).map_err(api_domain_error)?;
        if adopted_worktree.is_some() {
            let recovery = "The member explicitly chose to continue a failed Codex Run in its retained isolated workspace. Inspect and preserve the existing changes before editing. Continue toward the original request without redoing completed external side effects, and do not discard or reset existing changes.";
            project_instructions = Some(match project_instructions {
                Some(instructions) => format!("{instructions}\n\n{recovery}"),
                None => recovery.into(),
            });
        }
        executor
            .invoke_with_progress(
                WorkspaceAgentInvocation {
                    request_id: run.id.clone(),
                    model: run.config.model.clone(),
                    reasoning_effort: run.config.reasoning_effort.clone(),
                    project_instructions,
                    prompt,
                    commit_subject: user_text,
                    cwd: workdir,
                    adopted_worktree,
                    baseline_commit,
                    baseline_index_tree,
                    cancellation: control.cancellation.clone(),
                    integration_gate: Some(control),
                },
                progress,
            )
            .await
    }

    async fn supervise_workspace_agent(
        &self,
        run_id: String,
        control: Arc<WorkspaceRunControl>,
    ) -> Result<RunView, ApiError> {
        let worker = self.clone();
        let worker_run_id = run_id.clone();
        let task_control = Arc::clone(&control);
        let task = tokio::spawn(async move {
            worker
                .execute_workspace_agent(&worker_run_id, task_control)
                .await
        });
        let result = match task.await {
            Ok(Ok(run)) => Ok(run),
            Ok(Err(failure)) => {
                // Admission already made this Run visible to an asynchronous
                // caller. Every later error therefore belongs to the Run and
                // must be persisted before its Session and workspace leases
                // are released.
                self.finish_workspace_run(&run_id, Err(api_domain_error(failure)), None)
                    .await
            }
            Err(failure) => {
                self.finish_workspace_run(
                    &run_id,
                    Err(DomainError::invariant(
                        ErrorCode::ProviderFailed,
                        format!("workspace execution task failed: {failure}"),
                    )),
                    None,
                )
                .await
            }
        };
        drop(control);
        result
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

    async fn recovery_workspace(
        &self,
        run: &RunView,
    ) -> Result<Option<AdoptedWorkspace>, ApiError> {
        let Some(source_id) = run.recovery_of_run_id.as_ref() else {
            return Ok(None);
        };
        let output = self
            .store
            .load_run_outputs(&[source_id.as_str().to_owned()])
            .await
            .map_err(store_error)?
            .into_iter()
            .next()
            .ok_or_else(|| {
                error(
                    ErrorCode::RunNotResumable,
                    "the retained workspace archive is no longer available",
                    false,
                )
            })?;
        let worktree = output
            .output
            .worktree
            .filter(|worktree| worktree.dirty)
            .ok_or_else(|| {
                error(
                    ErrorCode::RunNotResumable,
                    "the retained workspace state is unknown or clean",
                    false,
                )
            })?;
        let path = worktree.retained_path.ok_or_else(|| {
            error(
                ErrorCode::RunNotResumable,
                "the failed Run has no retained isolated workspace",
                false,
            )
        })?;
        Ok(Some(AdoptedWorkspace {
            path: path.into(),
            fingerprint: worktree.fingerprint,
            source_run_id: worktree
                .retained_run_id
                .unwrap_or_else(|| source_id.as_str().to_owned()),
        }))
    }

    async fn set_run_settling(&self, run_id: &str) -> Result<bool, ApiError> {
        for _ in 0..4 {
            let snapshot = self.store.load().await.map_err(store_error)?;
            let mut state = decode_state(snapshot.value)?;
            let run = state
                .runs
                .iter_mut()
                .find(|run| run.id == run_id)
                .ok_or_else(|| error(ErrorCode::InvalidRun, "run not found", false))?;
            if run.status != "running" {
                return Ok(false);
            }
            run.status = "settling".into();
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
            "concurrent run settling update did not settle",
            true,
        ))
    }

    #[allow(
        clippy::too_many_lines,
        reason = "the terminal snapshot and immutable Message must stay visibly atomic"
    )]
    async fn finish_workspace_run(
        &self,
        run_id: &str,
        result: Result<WorkspaceAgentResponse, DomainError>,
        partial_output: Option<Box<RunPartialOutput>>,
    ) -> Result<RunView, ApiError> {
        let requested_output = partial_output.map(|output| *output);
        let mut persistence_failures = 0_u32;
        loop {
            let Ok(snapshot) = self.store.load().await else {
                wait_for_workspace_terminal_persistence(&mut persistence_failures).await;
                continue;
            };
            let mut state = decode_state(snapshot.value)?;
            let index = state
                .runs
                .iter()
                .position(|run| run.id == run_id)
                .ok_or_else(|| error(ErrorCode::InvalidRun, "run not found", false))?;
            let mut run = state.runs[index].clone();
            let embedded_output = run.partial_output.take().map(|output| *output);
            let terminal = matches!(
                run.status.as_str(),
                "completed" | "failed" | "cancelled" | "limit_exceeded"
            );
            let existing_output =
                if terminal && requested_output.is_none() && embedded_output.is_none() {
                    self.store
                        .load_run_outputs(&[run_id.to_owned()])
                        .await
                        .ok()
                        .and_then(|outputs| outputs.into_iter().next())
                        .map(|archive| archive.output)
                } else {
                    None
                };
            let output = requested_output
                .clone()
                .or(embedded_output)
                .or(existing_output);
            if terminal {
                state.runs[index] = run.clone();
                let mut public_run = run;
                public_run.partial_output = output.clone().map(Box::new);
                if requested_output.is_none() {
                    let _ = self.store.clear_progress(run_id).await;
                    return Ok(public_run);
                }
                // Terminal events carry only the lightweight Run metadata.
                // Readers hydrate the separately budgeted archive after refresh.
                let events = vec![terminal_run_event(&state.runs[index], output.is_some())];
                let value = serde_json::to_value(&state).map_err(serialization_error)?;
                let archive = output.map(|output| RunOutputArchive {
                    run_id: run_id.to_owned(),
                    output,
                    updated_at: now(),
                });
                match self
                    .store
                    .commit_terminal(snapshot.revision, value, events, run_id, archive)
                    .await
                {
                    Ok(_) => return Ok(public_run),
                    Err(ControlStoreError::Conflict | ControlStoreError::Other(_)) => {
                        wait_for_workspace_terminal_persistence(&mut persistence_failures).await;
                    }
                }
                continue;
            }
            apply_workspace_terminal_result(&mut state, &mut run, &result);
            release_session(&mut state, &run);
            run.partial_output = None;
            state.runs[index] = run.clone();
            let mut public_run = run;
            public_run.partial_output = output.clone().map(Box::new);
            let event = terminal_run_event(&state.runs[index], output.is_some());
            let value = serde_json::to_value(&state).map_err(serialization_error)?;
            let archive = output.map(|output| RunOutputArchive {
                run_id: run_id.to_owned(),
                output,
                updated_at: now(),
            });
            match self
                .store
                .commit_terminal(snapshot.revision, value, vec![event], run_id, archive)
                .await
            {
                Ok(_) => return Ok(public_run),
                Err(ControlStoreError::Conflict | ControlStoreError::Other(_)) => {
                    // A workspace adapter may already have made its Git result
                    // externally visible. Keep the supervisor's finalization
                    // control and Project/Session leases until the matching
                    // terminal Run, assistant output, and commit audit are all
                    // durable. Returning here would reopen cancellation and
                    // workspace admission around an unaudited integration.
                    wait_for_workspace_terminal_persistence(&mut persistence_failures).await;
                }
            }
        }
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
    pub async fn progress_checkpoints(&self) -> Result<Vec<Value>, ApiError> {
        self.store
            .load_progress()
            .await
            .map_err(store_error)
            .map(|values| values.into_iter().map(|value| value.body).collect())
    }

    /// Marks Runs left active by a previous daemon process as explicitly
    /// unrecovered. Automatic provider-thread resume is outside this slice.
    ///
    /// # Errors
    ///
    /// Returns a stable persistence error when recovery cannot be committed.
    pub async fn mark_interrupted_runs(&self) -> Result<usize, ApiError> {
        for _ in 0..4 {
            let snapshot = self.store.load().await.map_err(store_error)?;
            let mut state = decode_state(snapshot.value)?;
            let mut events = Vec::new();
            let mut interrupted = Vec::new();
            for run in &mut state.runs {
                if matches!(run.status.as_str(), "queued" | "running" | "settling") {
                    run.status = "failed".into();
                    run.error = Some(error(
                        ErrorCode::RunRecoveryFailed,
                        "the daemon restarted before this Run completed; automatic resume is not available",
                        false,
                    ));
                    interrupted.push(run.id.clone());
                    events.push(pending("run.updated", Some(run.id.clone()), run));
                }
            }
            if interrupted.is_empty() {
                return Ok(0);
            }
            for session in &mut state.sessions {
                if session
                    .active_run_id
                    .as_ref()
                    .is_some_and(|id| interrupted.contains(id))
                {
                    session.active_run_id = None;
                }
            }
            let count = interrupted.len();
            let value = serde_json::to_value(&state).map_err(serialization_error)?;
            match self.store.commit(snapshot.revision, value, events).await {
                Ok(_) => {
                    for run_id in interrupted {
                        let _ = self.store.clear_progress(&run_id).await;
                    }
                    return Ok(count);
                }
                Err(ControlStoreError::Conflict) => {}
                Err(error) => return Err(store_error(error)),
            }
        }
        Err(error(
            ErrorCode::RunQueueConflict,
            "interrupted Run recovery did not settle",
            true,
        ))
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
            let mut state = decode_state(snapshot.value)?;
            let only_run = match &command {
                Command::GetRun { run_id } => Some(run_id.as_str()),
                _ => None,
            };
            self.hydrate_run_outputs(&mut state, only_run).await?;
            return read_command(state, snapshot.revision, command);
        }

        // A Session request never waits behind a running turn: reject it immediately.
        let session_lease = self.acquire_session(&command)?;
        if let Command::SaveAgentProvider { provider, secret } = command {
            return self.save_provider(provider, secret).await;
        }
        if let Command::DiscoverProviderModels { provider, secret } = command {
            return self.discover_provider_models(provider, secret).await;
        }
        if let Command::RefreshProviderModels { provider_id } = command {
            return self.refresh_provider(&provider_id).await;
        }
        // Workspace-writing Codex requests for the same canonical Git root are
        // serialized before the user Message captures HEAD. Unrelated Projects
        // and read-only operations do not share this lease.
        let workspace_lease = self.acquire_workspace_write(&command).await?;
        let has_workspace_lease = workspace_lease.is_some();
        // Commit retries may reapply state changes, but never repeat an external
        // Agent invocation. Only the command that created the Run can request it.
        let outcome = self
            .commit_with_finalization_gate(command, has_workspace_lease)
            .await?;
        match outcome {
            CommandOutcome::Ready(result) => Ok(*result),
            CommandOutcome::ExecuteWorkspaceRun(run) => {
                let run_id = run.id.clone();
                let control = Arc::new(WorkspaceRunControl::new());
                let invocation = InvocationGuard::new(
                    Arc::clone(&self.cancellations),
                    &run_id,
                    control.cancellation.clone(),
                );
                let control_guard = WorkspaceRunControlGuard::new(
                    Arc::clone(&self.workspace_run_controls),
                    &run_id,
                    &control,
                );
                let service = self.clone();
                let (sender, receiver) = tokio::sync::oneshot::channel();
                tokio::spawn(async move {
                    // These guards deliberately live in the transport-independent
                    // supervisor until terminal persistence has completed.
                    let _owned = (session_lease, workspace_lease, invocation, control_guard);
                    let result = service.supervise_workspace_agent(run_id, control).await;
                    let _ = sender.send(result);
                });
                receiver
                    .await
                    .map_err(|_| {
                        error(
                            ErrorCode::ProviderFailed,
                            "workspace execution supervisor stopped before returning a result",
                            true,
                        )
                    })?
                    .map(CommandResult::Run)
            }
        }
    }

    async fn commit_with_finalization_gate(
        &self,
        command: Command,
        has_workspace_lease: bool,
    ) -> Result<CommandOutcome, ApiError> {
        let control = if let Command::CancelRun { run_id } = &command {
            self.workspace_run_controls
                .lock()
                .expect("workspace run controls")
                .get(run_id)
                .and_then(Weak::upgrade)
        } else {
            None
        };
        let Some(control) = control else {
            let outcome = self.commit_command(command, has_workspace_lease).await?;
            if let CommandOutcome::Ready(result) = &outcome
                && let CommandResult::Run(run) = result.as_ref()
                && run.status == "cancelled"
                && let Some(token) = self
                    .cancellations
                    .lock()
                    .expect("cancellations")
                    .get(&run.id)
            {
                token.cancel();
            }
            return Ok(outcome);
        };

        let mut decision = control.finalization.lock().await;
        if *decision == WorkspaceFinalizationDecision::Integrating {
            return Err(error(
                ErrorCode::RunAlreadyTerminal,
                "workspace integration has started; cancellation cannot replace its durable result",
                false,
            ));
        }
        let outcome = self.commit_command(command, has_workspace_lease).await?;
        if let CommandOutcome::Ready(result) = &outcome
            && let CommandResult::Run(run) = result.as_ref()
            && run.status == "cancelled"
        {
            *decision = WorkspaceFinalizationDecision::Cancelled;
            control.cancellation.cancel();
        }
        Ok(outcome)
    }

    async fn commit_command(
        &self,
        command: Command,
        has_workspace_lease: bool,
    ) -> Result<CommandOutcome, ApiError> {
        let archived_output = if let Command::ContinueRun { run_id, .. } = &command {
            self.store
                .load_run_outputs(std::slice::from_ref(run_id))
                .await
                .map_err(store_error)?
                .into_iter()
                .next()
                .map(|archive| archive.output)
        } else {
            None
        };
        for _ in 0..4 {
            let snapshot = self.store.load().await.map_err(store_error)?;
            let mut state = decode_state(snapshot.value)?;
            check_session_admission(&state, &command)?;
            validate_continue_worktree(&state, &command, archived_output.as_ref())?;
            if !has_workspace_lease && workspace_write_path(&state, &command)?.is_some() {
                return Err(error(
                    ErrorCode::ProjectWorkspaceBusy,
                    "Agent configuration changed to a workspace-writing provider during admission; retry the request",
                    true,
                ));
            }
            let git_baseline = command_git_baseline(&state, &command)?;
            let (result, events) =
                apply_command(&mut state, command.clone(), git_baseline.as_ref())?;
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

    async fn hydrate_run_outputs(
        &self,
        state: &mut State,
        only_run: Option<&str>,
    ) -> Result<(), ApiError> {
        // An empty selector asks storage for its independently bounded archive
        // catalog. This avoids serializing every historical Run id merely to
        // hydrate the small retained subset during a full snapshot read.
        let run_ids = only_run.map_or_else(Vec::new, |id| vec![id.to_owned()]);
        let outputs = self
            .store
            .load_run_outputs(&run_ids)
            .await
            .map_err(store_error)?;
        let outputs = outputs
            .into_iter()
            .map(|archive| (archive.run_id, Box::new(archive.output)))
            .collect::<HashMap<_, _>>();
        for run in &mut state.runs {
            if let Some(output) = outputs.get(&run.id) {
                run.partial_output = Some(output.clone());
            }
        }
        Ok(())
    }

    async fn acquire_workspace_write(
        &self,
        command: &Command,
    ) -> Result<Option<WorkspaceWriteLease>, ApiError> {
        let snapshot = self.store.load().await.map_err(store_error)?;
        let state = decode_state(snapshot.value)?;
        check_session_admission(&state, command)?;
        let Some(workdir) = workspace_write_path(&state, command)? else {
            return Ok(None);
        };
        let canonical = workdir.canonicalize().map_err(|failure| {
            error(
                ErrorCode::ProjectPathNotFound,
                format!("cannot resolve Project workdir for write admission: {failure}"),
                false,
            )
        })?;
        let process_lock = {
            let mut leases = self.workspace_leases.lock().map_err(|_| {
                error(
                    ErrorCode::ProjectWorkspaceBusy,
                    "workspace write lease registry is unavailable",
                    true,
                )
            })?;
            leases.retain(|_, lease| lease.strong_count() > 0);
            if let Some(existing) = leases.get(&canonical).and_then(Weak::upgrade) {
                existing
            } else {
                let lease = Arc::new(tokio::sync::Mutex::new(()));
                leases.insert(canonical.clone(), Arc::downgrade(&lease));
                lease
            }
        };
        let process_guard = process_lock.lock_owned().await;
        let git_dir = absolute_git_dir(&canonical)?;
        let lock_dir = git_dir.join("ait").join("locks");
        std::fs::create_dir_all(&lock_dir).map_err(|failure| {
            error(
                ErrorCode::ProjectWorkspaceBusy,
                format!("cannot create workspace lease directory: {failure}"),
                true,
            )
        })?;
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(lock_dir.join("workspace-write.lock"))
            .map_err(|failure| {
                error(
                    ErrorCode::ProjectWorkspaceBusy,
                    format!("cannot open workspace write lease: {failure}"),
                    true,
                )
            })?;
        match file.try_lock() {
            Ok(()) => Ok(Some(WorkspaceWriteLease {
                _process_guard: process_guard,
                _file: file,
            })),
            Err(std::fs::TryLockError::WouldBlock) => Err(error(
                ErrorCode::ProjectWorkspaceBusy,
                "another Ait process owns this Project workspace write lease",
                true,
            )),
            Err(std::fs::TryLockError::Error(failure)) => Err(error(
                ErrorCode::ProjectWorkspaceBusy,
                format!("cannot acquire Project workspace write lease: {failure}"),
                true,
            )),
        }
    }
}

fn workspace_write_path(state: &State, command: &Command) -> Result<Option<PathBuf>, ApiError> {
    let target = match command {
        Command::SendMessage { session_id, .. } => {
            let session = state
                .sessions
                .iter()
                .find(|session| session.id == *session_id)
                .ok_or_else(|| error(ErrorCode::SessionNotFound, "session not found", false))?;
            Some((session.project_id.as_str(), session.agent_id.as_str()))
        }
        Command::ForkSession {
            project_id,
            agent_id,
            ..
        } => Some((project_id.as_str(), agent_id.as_str())),
        Command::ContinueRun { run_id, .. } => {
            let source = state
                .runs
                .iter()
                .find(|run| run.id == *run_id)
                .ok_or_else(|| error(ErrorCode::InvalidRun, "run not found", false))?;
            Some((source.project_id.as_str(), source.agent_id.as_str()))
        }
        Command::TriggerCron {
            cron_id,
            scheduled_at,
        } => {
            if state.runs.iter().any(|run| {
                run.cron_id.as_deref() == Some(cron_id.as_str())
                    && run.scheduled_at == Some(*scheduled_at)
            }) {
                return Ok(None);
            }
            state
                .crons
                .iter()
                .find(|cron| cron.id == *cron_id && cron.enabled)
                .map(|cron| (cron.project_id.as_str(), cron.agent_id.as_str()))
        }
        _ => None,
    };
    let Some((project_id, agent_id)) = target else {
        return Ok(None);
    };
    let agent = require_agent(state, agent_id)?;
    if validate_config(state, &agent.config)?.kind != AgentMode::Codex {
        return Ok(None);
    }
    let project = state
        .projects
        .iter()
        .find(|project| project.id == project_id)
        .ok_or_else(|| error(ErrorCode::InvalidProject, "project not found", false))?;
    Ok(Some(PathBuf::from(&project.workdir)))
}

fn absolute_git_dir(workdir: &Path) -> Result<PathBuf, ApiError> {
    let output = ProcessCommand::new("git")
        .arg("-C")
        .arg(workdir)
        .args(["rev-parse", "--absolute-git-dir"])
        .output()
        .map_err(|failure| {
            error(
                ErrorCode::ProjectGitHeadUnavailable,
                format!("cannot locate Project Git directory: {failure}"),
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
    let path = PathBuf::from(String::from_utf8_lossy(&output.stdout).trim());
    if !path.is_absolute() {
        return Err(error(
            ErrorCode::ProjectGitHeadUnavailable,
            "Git returned a non-absolute metadata directory",
            false,
        ));
    }
    Ok(path)
}

fn api_domain_error(error: ApiError) -> DomainError {
    DomainError {
        code: error.code,
        message: error.message,
        retryable: error.retryable,
        details: None,
        cause_id: None,
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

#[allow(
    clippy::too_many_lines,
    reason = "Keep exhaustive command dispatch together"
)]
fn apply_command(
    state: &mut State,
    command: Command,
    user_git_baseline: Option<&GitBaseline>,
) -> Result<(CommandOutcome, Vec<PendingEvent>), ApiError> {
    let (result, events) = match command {
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
        Command::RegisterAgent { id, name, config } => register_agent(state, id, name, config),
        Command::UpdateAgent { id, name, config } => update_agent(state, &id, name, config),
        Command::SetSessionConfig { session_id, config } => {
            set_session_config(state, &session_id, config)
        }
        Command::SaveAgentProvider { .. }
        | Command::DiscoverProviderModels { .. }
        | Command::RefreshProviderModels { .. } => {
            unreachable!("provider command uses credential boundary")
        }
        Command::CreateSession {
            id,
            project_id,
            agent_id,
            at_message_id,
        } => create_session(state, id, project_id, &agent_id, at_message_id),
        Command::SetSessionAgent {
            session_id,
            agent_id,
        } => set_session_agent(state, &session_id, &agent_id),
        Command::RenameSession { session_id, name } => rename_session(state, &session_id, &name),
        Command::SetSessionTitle { session_id, title } => {
            set_session_title(state, &session_id, &title)
        }
        Command::SendMessage { session_id, text } => {
            return send_message(
                state,
                session_id,
                text,
                require_user_git_baseline(user_git_baseline)?,
            );
        }
        Command::ForkSession {
            id,
            project_id,
            agent_id,
            at_message_id,
            text,
        } => {
            return fork_session(
                state,
                ForkSessionInput {
                    id,
                    project_id,
                    agent_id,
                    at_message_id,
                    text,
                },
                require_user_git_baseline(user_git_baseline)?,
            );
        }
        Command::CancelRun { run_id } => cancel_run(state, &run_id),
        Command::ContinueRun {
            run_id,
            expected_worktree_fingerprint,
        } => {
            return continue_run(
                state,
                &run_id,
                &expected_worktree_fingerprint,
                require_user_git_baseline(user_git_baseline)?,
            );
        }
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
        } => return trigger_cron(state, &cron_id, scheduled_at, user_git_baseline),
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
    }?;
    Ok((CommandOutcome::Ready(Box::new(result)), events))
}

fn set_project_default_agent(
    state: &mut State,
    project_id: &str,
    agent_id: &str,
) -> Result<(CommandResult, Vec<PendingEvent>), ApiError> {
    require_named_agent(state, agent_id)?;
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
    git_baseline: &GitBaseline,
) -> Result<(CommandOutcome, Vec<PendingEvent>), ApiError> {
    let (_, mut events) = create_session(
        state,
        input.id.clone(),
        input.project_id,
        &input.agent_id,
        Some(input.at_message_id),
    )?;
    let (result, mut run_events) = send_message(state, input.id, input.text, git_baseline)?;
    events.append(&mut run_events);
    Ok((result, events))
}

fn require_user_git_baseline(baseline: Option<&GitBaseline>) -> Result<&GitBaseline, ApiError> {
    baseline.ok_or_else(|| {
        error(
            ErrorCode::ProjectGitHeadUnavailable,
            "Git HEAD/index snapshot is missing for user message",
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
    if id.trim().is_empty() || id.len() > MAX_CONTROL_ID_BYTES || name.trim().is_empty() {
        return Err(error(
            ErrorCode::InvalidProject,
            "project id must be non-empty and at most 256 bytes; name is required",
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
        created_at: now(),
        git_commit: None,
        data: None,
    });
    state.projects.push(project.clone());
    Ok((
        CommandResult::Project(project.clone()),
        vec![pending("project.registered", Some(id), &project)],
    ))
}

fn create_session(
    state: &mut State,
    id: String,
    project_id: String,
    agent_id: &str,
    at_message_id: Option<String>,
) -> Result<(CommandResult, Vec<PendingEvent>), ApiError> {
    if id.trim().is_empty()
        || id.len() > MAX_CONTROL_ID_BYTES
        || state.sessions.iter().any(|session| session.id == id)
    {
        return Err(error(
            ErrorCode::InvalidSession,
            "session id must be non-empty, unique, and at most 256 bytes",
            false,
        ));
    }
    let project = state
        .projects
        .iter()
        .find(|project| project.id == project_id)
        .ok_or_else(|| error(ErrorCode::InvalidProject, "project not found", false))?;
    require_agent(state, agent_id)?;
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
    let agent_id = agent_for_session(state, agent_id, &id)?;
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
) -> Result<(CommandResult, Vec<PendingEvent>), ApiError> {
    let agent_id = agent_for_session(state, agent_id, session_id)?;
    let session = state
        .sessions
        .iter_mut()
        .find(|session| session.id == session_id)
        .ok_or_else(|| error(ErrorCode::SessionNotFound, "session not found", false))?;
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
    session.agent_id = agent_id;
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
    git_baseline: &GitBaseline,
) -> Result<(CommandOutcome, Vec<PendingEvent>), ApiError> {
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
    if session.active_run_id.is_some() {
        return Err(error(
            ErrorCode::SessionBusy,
            "session already has an active run",
            false,
        ));
    }
    let agent = require_agent(state, &session.agent_id)?.clone();
    let provider = validate_config(state, &agent.config)?.clone();
    let user = message(
        &session.project_id,
        Some(&session.current_message_id),
        "user",
        "standard",
        Some(text),
        Some(&git_baseline.commit),
        None,
    );
    state.messages.push(user.clone());
    let run_id = Uuid::new_v4().to_string();
    state.sessions[index]
        .current_message_id
        .clone_from(&user.id);
    state.sessions[index].version += 1;
    state.sessions[index].active_run_id = Some(run_id.clone());
    let workspace_base_commit =
        (provider.kind == AgentMode::Codex).then(|| git_baseline.commit.clone());
    let workspace_base_index_tree = (provider.kind == AgentMode::Codex)
        .then(|| git_baseline.index_tree.clone().into_boxed_str());
    let run = RunView {
        id: run_id.clone(),
        project_id: session.project_id,
        base_message_id: user.id,
        last_message_id: None,
        session_id: Some(session_id),
        agent_id: agent.id.clone(),
        agent_revision: agent.revision,
        config: agent.config.clone(),
        provider,
        trigger: RunTrigger::Manual,
        cron_id: None,
        scheduled_at: None,
        workspace_base_commit,
        workspace_base_index_tree,
        status: "queued".into(),
        error: None,
        workspace_commit_id: None,
        partial_output: None,
        recovery_of_run_id: None,
    };
    if let Some(reference) = state.provider_credentials.get(&agent.config.provider_id) {
        state
            .run_credentials
            .insert(run_id.clone(), reference.clone());
    }
    state.runs.push(run);
    let run = state.runs.last().expect("new run exists").clone();
    let event = pending("run.updated", Some(run_id), &run);
    Ok((CommandOutcome::for_new_run(run), vec![event]))
}

fn codex_prompt(state: &State, head_id: &str) -> Result<(Option<String>, String), ApiError> {
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
    let mut instructions = Vec::new();
    let mut prompt = String::from("Conversation:\n");
    for message in path {
        if let Some(text) = &message.text {
            if message.role == "system" {
                instructions.push(text.as_str());
            } else {
                let _ = writeln!(prompt, "{}: {text}", message.role);
            }
        }
    }
    Ok((
        (!instructions.is_empty()).then(|| instructions.join("\n\n")),
        prompt,
    ))
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
        // Terminal Run transitions share one event contract so every client
        // schedules an authoritative snapshot refresh. Renderers still accept
        // legacy run.cancelled events retained in older outboxes.
        vec![pending("run.updated", Some(run.id.clone()), &run)],
    ))
}

fn validate_continue_worktree(
    state: &State,
    command: &Command,
    archived_output: Option<&RunPartialOutput>,
) -> Result<(), ApiError> {
    let Command::ContinueRun {
        run_id,
        expected_worktree_fingerprint,
    } = command
    else {
        return Ok(());
    };
    let run = state
        .runs
        .iter()
        .find(|run| run.id == *run_id)
        .ok_or_else(|| error(ErrorCode::InvalidRun, "run not found", false))?;
    if !matches!(
        run.status.as_str(),
        "failed" | "cancelled" | "limit_exceeded"
    ) || run.provider.kind != AgentMode::Codex
        || run.last_message_id.is_some()
    {
        return Err(error(
            ErrorCode::RunNotResumable,
            "only an unfinished terminal Codex Run can continue from workspace changes",
            false,
        ));
    }
    if state.runs.iter().any(|candidate| {
        candidate
            .recovery_of_run_id
            .as_ref()
            .is_some_and(|source| source.as_str() == run_id)
    }) {
        return Err(error(
            ErrorCode::RunNotResumable,
            "this Run already has a continuation",
            false,
        ));
    }
    let recorded = archived_output
        .or(run.partial_output.as_deref())
        .and_then(|partial| partial.worktree.as_ref())
        .filter(|worktree| worktree.dirty)
        .ok_or_else(|| {
            error(
                ErrorCode::RunNotResumable,
                "the Run has no recorded workspace changes to continue",
                false,
            )
        })?;
    if expected_worktree_fingerprint.is_empty()
        || expected_worktree_fingerprint != &recorded.fingerprint
    {
        return Err(error(
            ErrorCode::ProjectGitDirty,
            "the requested workspace snapshot does not match the failed Run",
            false,
        ));
    }
    let project = state
        .projects
        .iter()
        .find(|project| project.id == run.project_id)
        .ok_or_else(|| error(ErrorCode::InvalidProject, "project not found", false))?;
    let path = recorded.retained_path.as_deref().ok_or_else(|| {
        error(
            ErrorCode::RunNotResumable,
            "the Run has no retained isolated workspace",
            false,
        )
    })?;
    let inspection_path = Path::new(path).canonicalize().map_err(|failure| {
        error(
            ErrorCode::RunNotResumable,
            format!("cannot resolve the retained isolated workspace: {failure}"),
            false,
        )
    })?;
    let root = absolute_git_dir(Path::new(&project.workdir))?
        .join("ait")
        .join("workspaces");
    if !inspection_path.starts_with(&root) {
        return Err(error(
            ErrorCode::RunNotResumable,
            "the retained workspace is outside the manager-owned isolation root",
            false,
        ));
    }
    let current = inspect_worktree(&inspection_path)?;
    if !current.dirty || current.fingerprint != recorded.fingerprint {
        return Err(error(
            ErrorCode::ProjectGitDirty,
            "the Project worktree changed after this Run stopped; inspect it again before continuing",
            false,
        ));
    }
    Ok(())
}

fn continue_run(
    state: &mut State,
    source_run_id: &str,
    _expected_worktree_fingerprint: &str,
    workspace_baseline: &GitBaseline,
) -> Result<(CommandOutcome, Vec<PendingEvent>), ApiError> {
    let source = state
        .runs
        .iter()
        .find(|run| run.id == source_run_id)
        .cloned()
        .ok_or_else(|| error(ErrorCode::InvalidRun, "run not found", false))?;
    let session_id = source.session_id.clone().ok_or_else(|| {
        error(
            ErrorCode::RunNotResumable,
            "a Run without a Session cannot be continued interactively",
            false,
        )
    })?;
    let session_index = state
        .sessions
        .iter()
        .position(|session| session.id == session_id)
        .ok_or_else(|| error(ErrorCode::SessionNotFound, "session not found", false))?;
    let session = &state.sessions[session_index];
    if session.active_run_id.is_some() {
        return Err(error(
            ErrorCode::SessionBusy,
            "session already has an active run",
            false,
        ));
    }
    if session.current_message_id != source.base_message_id {
        return Err(error(
            ErrorCode::RunNotResumable,
            "the Session moved after the failed Run",
            false,
        ));
    }
    if source.workspace_base_commit.as_deref() != Some(workspace_baseline.commit.as_str())
        || source.workspace_base_index_tree.as_deref()
            != Some(workspace_baseline.index_tree.as_str())
    {
        return Err(error(
            ErrorCode::ProjectGitHeadUnavailable,
            "the Project Git baseline changed after the failed Run",
            false,
        ));
    }

    let run_id = Uuid::new_v4().to_string();
    state.sessions[session_index].active_run_id = Some(run_id.clone());
    state.sessions[session_index].version += 1;
    let run = RunView {
        id: run_id.clone(),
        project_id: source.project_id,
        base_message_id: source.base_message_id,
        last_message_id: None,
        session_id: Some(session_id),
        agent_id: source.agent_id,
        agent_revision: source.agent_revision,
        config: source.config,
        provider: source.provider,
        trigger: RunTrigger::Recovery,
        cron_id: None,
        scheduled_at: None,
        workspace_base_commit: Some(workspace_baseline.commit.clone()),
        workspace_base_index_tree: Some(workspace_baseline.index_tree.clone().into_boxed_str()),
        status: "queued".into(),
        error: None,
        workspace_commit_id: None,
        partial_output: None,
        recovery_of_run_id: Some(RunId::new(source_run_id)),
    };
    if let Some(reference) = state.run_credentials.get(source_run_id).cloned() {
        state.run_credentials.insert(run_id.clone(), reference);
    }
    state.runs.push(run.clone());
    Ok((
        CommandOutcome::for_new_run(run.clone()),
        vec![pending("run.recovery_started", Some(run_id), &run)],
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
    require_named_agent(state, &agent_id).map_err(|_| {
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
    workspace_baseline: Option<&GitBaseline>,
) -> Result<(CommandOutcome, Vec<PendingEvent>), ApiError> {
    if let Some(existing) = state.runs.iter().find(|run| {
        run.cron_id.as_deref() == Some(cron_id) && run.scheduled_at == Some(scheduled_at)
    }) {
        return Ok((
            CommandOutcome::Ready(Box::new(CommandResult::Run(existing.clone()))),
            Vec::new(),
        ));
    }
    let cron = state
        .crons
        .iter()
        .find(|cron| cron.id == cron_id && cron.enabled)
        .cloned()
        .ok_or_else(|| error(ErrorCode::InvalidCron, "enabled cron not found", false))?;
    let agent = require_agent(state, &cron.agent_id)?.clone();
    let provider = validate_config(state, &agent.config)?.clone();
    if provider.kind == AgentMode::Codex && workspace_baseline.is_none() {
        return Err(error(
            ErrorCode::ProjectGitHeadUnavailable,
            "Codex Cron Run requires a Git baseline captured under the workspace lease",
            true,
        ));
    }
    let run_id = Uuid::new_v4().to_string();
    if let Some(reference) = state.provider_credentials.get(&agent.config.provider_id) {
        state
            .run_credentials
            .insert(run_id.clone(), reference.clone());
    }
    state.runs.push(RunView {
        id: run_id.clone(),
        project_id: cron.project_id,
        base_message_id: cron.base_message_id,
        last_message_id: None,
        session_id: None,
        agent_id: agent.id.clone(),
        agent_revision: agent.revision,
        config: agent.config.clone(),
        provider,
        trigger: RunTrigger::Cron,
        cron_id: Some(cron.id),
        scheduled_at: Some(scheduled_at),
        workspace_base_commit: workspace_baseline.map(|baseline| baseline.commit.clone()),
        workspace_base_index_tree: workspace_baseline
            .map(|baseline| baseline.index_tree.clone().into_boxed_str()),
        status: "queued".into(),
        error: None,
        workspace_commit_id: None,
        partial_output: None,
        recovery_of_run_id: None,
    });
    let run = state.runs.last().expect("new run exists").clone();
    let event = pending("cron.run_triggered", Some(run_id), &run);
    Ok((CommandOutcome::for_new_run(run), vec![event]))
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
    let agents: Vec<_> = state
        .agents
        .iter()
        .filter(|agent| referenced_agents.contains(agent.id.as_str()))
        .cloned()
        .collect();
    let providers = state
        .providers
        .iter()
        .filter(|p| agents.iter().any(|a| a.config.provider_id == p.provider.id))
        .map(|p| p.provider.clone())
        .collect();
    let archive = ProjectExport {
        format_version: PROJECT_EXPORT_VERSION,
        source_revision,
        providers,
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

    for provider in &archive.providers {
        if let Some(existing) = state
            .providers
            .iter()
            .find(|p| p.provider.id == provider.id)
            && existing.provider != *provider
        {
            return Err(invalid_archive(
                "archive provider conflicts with an existing connection",
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
    for provider in archive.providers {
        if !state.providers.iter().any(|p| p.provider.id == provider.id) {
            state.providers.push(AgentProviderView {
                provider,
                has_secret: false,
            });
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
        || archive.project.id.len() > MAX_CONTROL_ID_BYTES
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

    validate_archive_catalog(archive)?;
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
        if session.id.trim().is_empty()
            || session.id.len() > MAX_CONTROL_ID_BYTES
            || session.project_id != archive.project.id
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
        created_at: now(),
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

fn command_git_baseline(state: &State, command: &Command) -> Result<Option<GitBaseline>, ApiError> {
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
        Command::ContinueRun { run_id, .. } => Some(
            state
                .runs
                .iter()
                .find(|run| run.id == *run_id)
                .ok_or_else(|| error(ErrorCode::InvalidRun, "run not found", false))?
                .project_id
                .as_str(),
        ),
        Command::TriggerCron {
            cron_id,
            scheduled_at,
        } => {
            if state.runs.iter().any(|run| {
                run.cron_id.as_deref() == Some(cron_id.as_str())
                    && run.scheduled_at == Some(*scheduled_at)
            }) {
                return Ok(None);
            }
            let Some(cron) = state
                .crons
                .iter()
                .find(|cron| cron.id == *cron_id && cron.enabled)
            else {
                return Ok(None);
            };
            let agent = require_agent(state, &cron.agent_id)?;
            if validate_config(state, &agent.config)?.kind != AgentMode::Codex {
                return Ok(None);
            }
            Some(cron.project_id.as_str())
        }
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
    clean_git_baseline(Path::new(&project.workdir)).map(Some)
}

fn clean_git_baseline(path: &Path) -> Result<GitBaseline, ApiError> {
    let before = git_head(path)?.ok_or_else(|| {
        error(
            ErrorCode::ProjectGitHeadUnavailable,
            "project repository has no HEAD commit",
            false,
        )
    })?;
    let index_before = git_index_tree(path)?;
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
    let index_after = git_index_tree(path)?;
    if index_before != index_after {
        return Err(error(
            ErrorCode::ProjectGitDirty,
            "project Git index changed while adding a user message; retry",
            true,
        ));
    }
    let head_tree = git_commit_tree(path, &after)?;
    if index_after != head_tree {
        return Err(error(
            ErrorCode::ProjectGitDirty,
            "project Git index does not match HEAD at write admission",
            false,
        ));
    }
    Ok(GitBaseline {
        commit: after,
        index_tree: index_after,
    })
}

fn git_index_tree(path: &Path) -> Result<String, ApiError> {
    let output = ProcessCommand::new("git")
        .arg("-C")
        .arg(path)
        .arg("write-tree")
        .output()
        .map_err(|failure| {
            error(
                ErrorCode::ProjectGitHeadUnavailable,
                format!("cannot snapshot Project Git index: {failure}"),
                false,
            )
        })?;
    if !output.status.success() {
        return Err(error(
            ErrorCode::ProjectGitDirty,
            String::from_utf8_lossy(&output.stderr).trim(),
            false,
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn git_commit_tree(path: &Path, commit: &str) -> Result<String, ApiError> {
    let expression = format!("{commit}^{{tree}}");
    let output = ProcessCommand::new("git")
        .arg("-C")
        .arg(path)
        .args(["rev-parse", "--verify", &expression])
        .output()
        .map_err(|failure| {
            error(
                ErrorCode::ProjectGitHeadUnavailable,
                format!("cannot resolve Project Git commit tree: {failure}"),
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
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

const MAX_WORKTREE_CHANGES: usize = 256;
const MAX_WORKTREE_PATH_CHARS: usize = 128;

fn terminal_partial_output(
    progress: FinishedProgress,
    workdir: &Path,
    failure: Option<&DomainError>,
) -> Result<Option<Box<RunPartialOutput>>, ApiError> {
    let has_progress = progress.checkpoint.as_ref().is_some_and(|checkpoint| {
        checkpoint["items"]
            .as_array()
            .is_some_and(|items| !items.is_empty())
            || checkpoint["warnings"]
                .as_array()
                .is_some_and(|warnings| !warnings.is_empty())
    });
    let retained = failure.and_then(retained_workspace_details);
    let inspection_path = retained
        .as_ref()
        .map_or(workdir, |(path, _)| path.as_path());
    let (worktree, worktree_error) = match inspect_worktree(inspection_path) {
        Ok(mut state) => {
            if let Some((path, owner_run_id)) = retained {
                state.retained_path = Some(path.to_string_lossy().into_owned());
                state.retained_run_id = Some(owner_run_id);
            }
            (Some(state), None)
        }
        Err(failure) => (
            None,
            Some(DomainError {
                code: failure.code,
                message: failure.message,
                retryable: failure.retryable,
                details: None,
                cause_id: None,
            }),
        ),
    };
    let has_changes = worktree.as_ref().is_some_and(|state| state.dirty);
    let progress_error = progress
        .persistence_error
        .map(|failure| {
            DomainError::invariant(
                ErrorCode::RunRecoveryFailed,
                bounded_chars(&failure, 8 * 1024),
            )
        })
        .or_else(|| {
            progress.checkpoint.is_none().then(|| {
                DomainError::invariant(
                    ErrorCode::RunRecoveryFailed,
                    "final progress checkpoint is unavailable",
                )
            })
        });
    if !(has_progress || has_changes || progress_error.is_some() || worktree_error.is_some()) {
        return Ok(None);
    }
    let output = RunPartialOutput {
        progress: has_progress.then_some(progress.checkpoint).flatten(),
        progress_error,
        worktree,
        worktree_error,
    };
    output
        .validate()
        .map_err(|failure| error(failure.code, failure.message, failure.retryable))?;
    Ok(Some(Box::new(output)))
}

fn retained_workspace_details(failure: &DomainError) -> Option<(PathBuf, String)> {
    let details = failure.details.as_ref()?;
    let path = details.0.get("retained_worktree_path")?.as_str()?;
    let owner_run_id = details.0.get("retained_source_run_id")?.as_str()?;
    Some((PathBuf::from(path), owner_run_id.to_owned()))
}

fn inspect_worktree(path: &Path) -> Result<RunWorktreeState, ApiError> {
    let snapshot = ait_worktree::inspect(path).map_err(|failure| {
        error(
            ErrorCode::ProjectGitHeadUnavailable,
            failure.to_string(),
            false,
        )
    })?;
    let records = snapshot
        .status
        .split(|byte| *byte == 0)
        .filter(|record| !record.is_empty())
        .collect::<Vec<_>>();
    let mut changes = Vec::new();
    let mut index = 0;
    let mut total = 0;
    while index < records.len() {
        let record = records[index];
        if record.len() < 3 || record[2] != b' ' {
            index += 1;
            continue;
        }
        total += 1;
        if changes.len() < MAX_WORKTREE_CHANGES {
            changes.push(RunWorktreeChange {
                status: String::from_utf8_lossy(&record[..2]).into_owned(),
                path: bounded_chars(
                    &String::from_utf8_lossy(&record[3..]),
                    MAX_WORKTREE_PATH_CHARS,
                ),
            });
        }
        let renamed = record[..2]
            .iter()
            .any(|status| matches!(*status, b'R' | b'C'));
        index += if renamed { 2 } else { 1 };
    }
    Ok(RunWorktreeState {
        retained_path: None,
        retained_run_id: None,
        head: snapshot.head,
        dirty: !snapshot.status.is_empty(),
        fingerprint: snapshot.fingerprint,
        changes,
        truncated: total > MAX_WORKTREE_CHANGES,
    })
}

fn bounded_chars(value: &str, limit: usize) -> String {
    if value.chars().count() <= limit {
        value.to_owned()
    } else {
        value.chars().take(limit).collect()
    }
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

async fn wait_for_workspace_terminal_persistence(failures: &mut u32) {
    let exponent = (*failures).min(8);
    let delay_ms = 1_u64 << exponent;
    *failures = failures.saturating_add(1);
    tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
}

fn apply_workspace_terminal_result(
    state: &mut State,
    run: &mut RunView,
    result: &Result<WorkspaceAgentResponse, DomainError>,
) {
    match result {
        Ok(output) => {
            let operations = output
                .operations
                .iter()
                .map(|operation| {
                    json!({
                        "id": operation.id,
                        "kind": operation.kind,
                        "status": operation.status,
                        "title": operation.title,
                        "summary": operation.summary,
                        "detail": operation.detail,
                        "paths": operation.paths,
                    })
                })
                .collect::<Vec<_>>();
            let output_items = output
                .output_items
                .iter()
                .map(|item| match item {
                    WorkspaceOutputItem::Message { id, phase, text } => json!({
                        "type": "message",
                        "id": id,
                        "phase": phase,
                        "text": text,
                    }),
                    WorkspaceOutputItem::Operation { id } => json!({
                        "type": "operation",
                        "id": id,
                    }),
                })
                .collect::<Vec<_>>();
            let data =
                (output.commit_id.is_some() || !operations.is_empty() || !output_items.is_empty())
                    .then(|| {
                        json!({"codex":{
                            "commit_id": output.commit_id,
                            "operations": operations,
                            "output_items": output_items,
                        }})
                    });
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
            append_output(state, run, reply);
            run.status = "completed".into();
            run.error = None;
        }
        Err(failure) => {
            run.status = match failure.code {
                ErrorCode::RunCancelled => "cancelled",
                ErrorCode::RunLimitExceeded => "limit_exceeded",
                _ => "failed",
            }
            .into();
            run.error = Some(error(failure.code, &failure.message, failure.retryable));
        }
    }
}

fn pending<T: Serialize>(kind: &str, entity_id: Option<String>, body: &T) -> PendingEvent {
    PendingEvent {
        kind: kind.into(),
        entity_id,
        body: serde_json::to_value(body).unwrap_or(Value::Null),
        created_at: now(),
    }
}

fn terminal_run_event(run: &RunView, partial_output_available: bool) -> PendingEvent {
    pending(
        "run.updated",
        Some(run.id.clone()),
        &json!({
            "version": 1,
            "id": run.id,
            "status": run.status,
            "partial_output_available": partial_output_available,
        }),
    )
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
        let mut state: State =
            serde_json::from_value(migrate_state(value)?).map_err(serialization_error)?;
        state.settings.0.retain(|id, _| !id.starts_with("models."));
        Ok(state)
    }
}

fn validate_archive_catalog(archive: &ProjectExport) -> Result<(), ApiError> {
    let mut provider_ids = HashSet::new();
    for provider in &archive.providers {
        validate_provider(provider)?;
        if !provider_ids.insert(&provider.id) {
            return Err(invalid_archive("duplicate provider"));
        }
    }
    for agent in &archive.agents {
        // Archives preserve configuration even when a provider has delisted its model.
        // Availability is checked when starting new work, not when copying history.
        if !provider_ids.contains(&agent.config.provider_id)
            || agent.config.model.trim().is_empty()
            || agent
                .config
                .reasoning_effort
                .as_ref()
                .is_some_and(|effort| effort.trim().is_empty())
        {
            return Err(invalid_archive(
                "invalid archived Agent configuration or provider reference",
            ));
        }
        if let Some(owner) = &agent.owner_session_id {
            if !agent.name.is_empty()
                || !archive
                    .sessions
                    .iter()
                    .any(|s| &s.id == owner && s.agent_id == agent.id)
            {
                return Err(invalid_archive("invalid anonymous Agent owner"));
            }
        } else if agent.name.trim().is_empty() {
            return Err(invalid_archive("named Agent requires a name"));
        }
    }
    if archive.project.default_agent_id.as_ref().is_some_and(|id| {
        archive
            .agents
            .iter()
            .any(|a| &a.id == id && a.owner_session_id.is_some())
    }) {
        return Err(invalid_archive(
            "Project default Agent must be a named preset",
        ));
    }
    for session in &archive.sessions {
        if archive.agents.iter().any(|a| {
            a.id == session.agent_id
                && a.owner_session_id
                    .as_ref()
                    .is_some_and(|owner| owner != &session.id)
        }) {
            return Err(invalid_archive(
                "anonymous Agent cannot be shared between Sessions",
            ));
        }
    }
    Ok(())
}
