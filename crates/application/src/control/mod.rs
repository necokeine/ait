//! Shared service composition, public command entry points and exhaustive command routing.
#![allow(missing_docs)]

use crate::control::approvals::resolve_native_approval;
use crate::control::catalog::{register_agent, set_session_config, update_agent};
use crate::control::conversation::messages::send_message;
use crate::control::conversation::title::set_session_title;
use crate::control::conversation::{
    ForkSessionInput, create_session, derive_session, fork_session, rename_session,
    set_session_agent,
};
use crate::control::cron::{create_cron, set_cron_enabled, trigger_cron};
use crate::control::errors::error;
use crate::control::execution::CommandOutcome;
use crate::control::project::archive::{export_project, import_project};
use crate::control::project::git::{GitBaseline, require_user_git_baseline};
use crate::control::project::{register_project, set_project_default_agent};
use crate::control::runs::cancel_run;
use crate::control::runs::finalization::WorkspaceRunControl;
use crate::control::settings::{reset_settings, save_settings, settings_view};
use crate::control::state::WorkingSet;
use ait_contracts::{ApiError, Command, CommandResult, Response};
use ait_domain::ErrorCode;
use ait_ports::{
    AgentProviderGateway, ControlStore, HostProviderModelCatalog, PendingEvent,
    ProjectDirectoryCreator, SessionTitleGenerator, WorkspaceAgent, WorkspaceApprovalDecision,
};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, Weak};

pub(in crate::control) mod admission;
pub(in crate::control) mod approvals;
pub(in crate::control) mod catalog;
pub(in crate::control) mod conversation;
pub(in crate::control) mod cron;
pub(in crate::control) mod errors;
pub(in crate::control) mod events;
pub(in crate::control) mod execution;
pub(in crate::control) mod permissions;
pub(in crate::control) mod project;
pub(in crate::control) mod runs;
pub(in crate::control) mod settings;
pub(in crate::control) mod state;

pub use permissions::PermissionPolicyLimits;
pub use runs::recovery::StartupRecoveryPlan;

/// Shared application entry point used by every transport adapter.
#[derive(Clone)]
pub struct LocalControlService {
    store: Arc<dyn ControlStore>,
    project_directory_creator: Option<Arc<dyn ProjectDirectoryCreator>>,
    session_leases: Arc<Mutex<HashMap<String, Weak<()>>>>,
    workspace_leases: Arc<Mutex<HashMap<PathBuf, Weak<tokio::sync::Mutex<()>>>>>,
    cancellations: Arc<Mutex<HashMap<String, tokio_util::sync::CancellationToken>>>,
    workspace_run_controls: Arc<Mutex<HashMap<String, Weak<WorkspaceRunControl>>>>,
    approval_waiters:
        Arc<Mutex<HashMap<String, tokio::sync::watch::Sender<Option<WorkspaceApprovalDecision>>>>>,
    permission_limits: PermissionPolicyLimits,
    provider_gateway: Option<Arc<dyn AgentProviderGateway>>,
    api_tools: Option<Arc<dyn ait_ports::RunToolFactory>>,
    run_dispatcher: Option<Arc<dyn ait_ports::RunDispatcher>>,
    draining: Arc<AtomicBool>,
    admission: Arc<tokio::sync::RwLock<()>>,
    host_provider_catalog: Option<Arc<dyn HostProviderModelCatalog>>,
    workspace_agent: Option<Arc<dyn WorkspaceAgent>>,
    session_title_generator: Option<Arc<dyn SessionTitleGenerator>>,
}

impl LocalControlService {
    #[must_use]
    pub fn new(store: Arc<dyn ControlStore>) -> Self {
        Self {
            store,
            project_directory_creator: None,
            session_leases: Arc::new(Mutex::new(HashMap::new())),
            workspace_leases: Arc::new(Mutex::new(HashMap::new())),
            cancellations: Arc::new(Mutex::new(HashMap::new())),
            workspace_run_controls: Arc::new(Mutex::new(HashMap::new())),
            approval_waiters: Arc::new(Mutex::new(HashMap::new())),
            permission_limits: PermissionPolicyLimits::default(),
            provider_gateway: None,
            api_tools: None,
            run_dispatcher: None,
            draining: Arc::new(AtomicBool::new(false)),
            admission: Arc::new(tokio::sync::RwLock::new(())),
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
            project_directory_creator: None,
            session_leases: Arc::new(Mutex::new(HashMap::new())),
            workspace_leases: Arc::new(Mutex::new(HashMap::new())),
            cancellations: Arc::new(Mutex::new(HashMap::new())),
            workspace_run_controls: Arc::new(Mutex::new(HashMap::new())),
            approval_waiters: Arc::new(Mutex::new(HashMap::new())),
            permission_limits: PermissionPolicyLimits::default(),
            provider_gateway: None,
            api_tools: None,
            run_dispatcher: None,
            draining: Arc::new(AtomicBool::new(false)),
            admission: Arc::new(tokio::sync::RwLock::new(())),
            host_provider_catalog: None,
            workspace_agent: Some(workspace_agent),
            session_title_generator: None,
        }
    }

    /// Installs the host tool executor factory used only by API Providers.
    #[must_use]
    pub fn with_api_tools(mut self, factory: Arc<dyn ait_ports::RunToolFactory>) -> Self {
        self.api_tools = Some(factory);
        self
    }

    /// Installs supervised execution for all API Runs, including startup recovery.
    #[must_use]
    pub fn with_run_dispatcher(mut self, dispatcher: Arc<dyn ait_ports::RunDispatcher>) -> Self {
        self.run_dispatcher = Some(dispatcher);
        self
    }

    /// Adds the host capability used only when Project registration omits a workdir.
    #[must_use]
    pub fn with_project_directory_creator(
        mut self,
        creator: Arc<dyn ProjectDirectoryCreator>,
    ) -> Self {
        self.project_directory_creator = Some(creator);
        self
    }

    /// Installs the credential and API completion gateway.
    #[must_use]
    pub fn with_provider_gateway(mut self, gateway: Arc<dyn AgentProviderGateway>) -> Self {
        self.provider_gateway = Some(gateway);
        self
    }

    /// Applies an administrator-owned upper bound to subsequent Run admission.
    #[must_use]
    pub fn with_permission_limits(mut self, limits: PermissionPolicyLimits) -> Self {
        self.permission_limits = limits;
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
}

pub(in crate::control) fn read_command(
    state: WorkingSet,
    revision: u64,
    command: Command,
) -> Result<CommandResult, ApiError> {
    match command {
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
        Command::ListProjects => Ok(CommandResult::Projects(state.projects)),
        Command::ListAgents => Ok(CommandResult::Agents(state.agents)),
        Command::ListAgentProviders => Ok(CommandResult::AgentProviders(state.providers)),
        Command::ListSessions { .. } => Ok(CommandResult::Sessions(state.sessions)),
        Command::ListMessages { .. } => Ok(CommandResult::Messages(state.messages)),
        Command::ListRuns { .. } => Ok(CommandResult::Runs(state.runs)),
        Command::ListCrons => Ok(CommandResult::Crons(state.crons)),
        _ => unreachable!("mutating command routed to read path"),
    }
}

#[allow(
    clippy::too_many_lines,
    reason = "Keep exhaustive command dispatch together"
)]
pub(in crate::control) fn apply_command(
    state: &mut WorkingSet,
    command: Command,
    user_git_baseline: Option<&GitBaseline>,
    permission_limits: PermissionPolicyLimits,
    derive_source_locked: bool,
) -> Result<(CommandOutcome, Vec<PendingEvent>), ApiError> {
    let (result, events) = match command {
        Command::RegisterProject {
            id,
            name,
            workdir,
            repo_url,
        } => register_project(
            state,
            id,
            name,
            workdir
                .as_deref()
                .expect("workdir prepared before dispatch"),
            repo_url,
        ),
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
                permission_limits,
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
                permission_limits,
            );
        }
        Command::DeriveSession {
            id,
            project_id,
            source_session_id,
            agent_id,
            at_message_id,
            text,
        } => {
            return derive_session(
                state,
                ForkSessionInput {
                    id,
                    project_id,
                    agent_id,
                    at_message_id,
                    text,
                },
                &source_session_id,
                derive_source_locked,
                require_user_git_baseline(user_git_baseline)?,
                permission_limits,
            );
        }
        Command::CancelRun { run_id } => cancel_run(state, &run_id),
        Command::ResolveNativeApproval {
            run_id,
            approval_id,
            action,
            scope,
        } => resolve_native_approval(
            state,
            &run_id,
            &approval_id,
            action,
            scope,
            permission_limits,
        ),
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
        } => {
            return trigger_cron(
                state,
                &cron_id,
                scheduled_at,
                user_git_baseline,
                permission_limits,
            );
        }
        Command::ImportProject { archive, workdir } => import_project(state, archive, &workdir),
        Command::SaveSettings {
            expected_revision,
            values,
        } => save_settings(state, expected_revision, values),
        Command::ResetSettings => Ok(reset_settings(state)),
        Command::GetRun { .. }
        | Command::ExportProject { .. }
        | Command::GetSettings
        | Command::ListProjects
        | Command::ListAgents
        | Command::ListAgentProviders
        | Command::ListSessions { .. }
        | Command::ListMessages { .. }
        | Command::ListRuns { .. }
        | Command::ListCrons => unreachable!("read command routed to write path"),
    }?;
    Ok((CommandOutcome::Ready(Box::new(result)), events))
}
