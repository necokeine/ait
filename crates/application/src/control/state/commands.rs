//! Exhaustive typed command dispatch. Each arm exposes only its context.
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
use crate::control::model::{CronState, ProjectState, SessionState};
use crate::control::project::archive::{
    export_project, import_project, validate_import_conflicts, validate_project_export,
};
use crate::control::project::git::{GitBaseline, PreparedProject, require_user_git_baseline};
use crate::control::project::{
    register_project, set_project_default_agent, update_project, validate_project_workdir,
};
use crate::control::runs::cancel_run;

use crate::control::errors::store_error;
use crate::control::permissions::PermissionPolicyLimits;
use crate::control::settings::{reset_settings, save_settings, settings_view};
use crate::control::state::transaction::RecordTransaction;
use crate::control::state::{
    AgentContext, AgentsContext, ArchiveContext, ConversationContext, CronCreateContext,
    CronTriggerContext, CronsContext, MessagesContext, NewSessionContext, ProjectAgentContext,
    ProjectRegistrationContext, ProjectsContext, ProviderContext, RunControlContext, RunsContext,
    SessionBindingContext, SessionConfigContext, SessionsContext, SettingsContext,
};
use ait_contracts::{ApiError, Command, CommandResult};
use ait_domain::ErrorCode;
use ait_ports::{ControlChange, PendingEvent, ProjectWorkspace, WorkspaceLease};
use std::sync::Arc;

pub(in crate::control) enum CommandTransaction {
    NewSession(RecordTransaction<NewSessionContext>),
    SessionBinding(RecordTransaction<SessionBindingContext>),
    Agent(RecordTransaction<AgentContext>),
    Agents(RecordTransaction<AgentsContext>),
    Archive(RecordTransaction<ArchiveContext>),
    Conversation(RecordTransaction<ConversationContext>),
    CronCreate(RecordTransaction<CronCreateContext>),
    CronTrigger(RecordTransaction<CronTriggerContext>),
    Crons(RecordTransaction<CronsContext>),
    Messages(RecordTransaction<MessagesContext>),
    ProjectAgent(RecordTransaction<ProjectAgentContext>),
    ProjectRegistration(RecordTransaction<ProjectRegistrationContext>),
    Projects(RecordTransaction<ProjectsContext>),
    Provider(RecordTransaction<ProviderContext>),
    RunControl(RecordTransaction<RunControlContext>),
    Runs(RecordTransaction<RunsContext>),
    SessionConfig(RecordTransaction<SessionConfigContext>),
    Sessions(RecordTransaction<SessionsContext>),
    Settings(RecordTransaction<SettingsContext>),
}
pub(in crate::control) struct CommandCommit {
    pub(in crate::control) revision: u64,
    pub(in crate::control) changes: Vec<ControlChange>,
    pub(in crate::control) events: Vec<PendingEvent>,
    pub(in crate::control) outcome: CommandOutcome,
}
impl CommandTransaction {
    pub(in crate::control) fn check_admission(&self, command: &Command) -> Result<(), ApiError> {
        match self {
            Self::ProjectRegistration(tx) => {
                let Command::RegisterProject {
                    id,
                    name,
                    workdir,
                    repo_url,
                } = command
                else {
                    unreachable!("registration context")
                };
                self.validate_registration(id, name, &mut repo_url.clone())?;
                // Workspace admission can run before a name-only Project has
                // allocated its directory. Commit admission rechecks the resolved path.
                if let Some(workdir) = workdir {
                    validate_project_workdir(&tx.original, workdir)?;
                }
                Ok(())
            }
            Self::Archive(tx) => {
                if let Command::ImportProject { archive, workdir } = command {
                    validate_project_export(archive)?;
                    validate_import_conflicts(&tx.original, archive)?;
                    validate_project_workdir(&tx.original, workdir)?;
                }
                Ok(())
            }
            Self::Conversation(tx) => {
                crate::control::admission::check_session_admission(&tx.original, command)
            }
            Self::SessionConfig(tx) => {
                crate::control::admission::check_session_admission(&tx.original, command)
            }
            Self::NewSession(tx) => {
                crate::control::admission::check_session_admission(&tx.original, command)
            }
            Self::SessionBinding(tx) => {
                crate::control::admission::check_session_admission(&tx.original, command)
            }
            _ => Ok(()),
        }
    }
    pub(in crate::control) fn validate_registration(
        &self,
        id: &str,
        name: &str,
        repo_url: &mut Option<String>,
    ) -> Result<(), ApiError> {
        let Self::ProjectRegistration(tx) = self else {
            unreachable!("registration context")
        };
        crate::control::project::validate_project_registration(&tx.original, id, name, repo_url)
    }
    pub(in crate::control) fn workspace_path(
        &self,
        command: &Command,
        limits: PermissionPolicyLimits,
    ) -> Result<Option<std::path::PathBuf>, ApiError> {
        match self {
            Self::Conversation(tx) => {
                crate::control::admission::workspace_write_path(&tx.original, command, limits)
            }
            Self::NewSession(tx) => {
                crate::control::admission::workspace_write_path(&tx.original, command, limits)
            }
            Self::CronTrigger(tx) => {
                crate::control::admission::cron_workspace_write_path(&tx.original, command, limits)
            }
            _ => Ok(None),
        }
    }
    pub(in crate::control) async fn prepare(
        &self,
        workspace: &dyn ProjectWorkspace,
        lease: Option<Arc<dyn WorkspaceLease>>,
        command: &Command,
        prepared_project: Option<&PreparedProject>,
        created: &mut Vec<std::path::PathBuf>,
    ) -> Result<(), ApiError> {
        match self {
            Self::Conversation(tx) => {
                crate::control::project::worktrees::prepare_command_session_worktrees(
                    workspace,
                    lease,
                    &tx.original,
                    command,
                    created,
                )
                .await
            }
            Self::Archive(tx) => {
                let Command::ImportProject { archive, .. } = command else {
                    unreachable!("import preparation")
                };
                crate::control::project::worktrees::prepare_import_session_worktrees(
                    workspace,
                    lease,
                    &tx.original,
                    archive,
                    prepared_project.expect("Project prepared before Session worktrees"),
                    created,
                )
                .await
            }
            Self::NewSession(tx) => {
                let Command::CreateSession {
                    id,
                    project_id,
                    agent_id,
                    at_message_id,
                } = command
                else {
                    unreachable!("new Session")
                };
                let project =
                    crate::control::project::require_project_view(&tx.original, project_id)?;
                let agent_id = crate::control::settings::resolve_project_agent_id(
                    &tx.original,
                    project_id,
                    agent_id,
                )?;
                crate::control::project::worktrees::prepare_new_session_worktree(
                    workspace,
                    lease,
                    &tx.original,
                    id,
                    project_id,
                    &agent_id,
                    at_message_id.as_deref().unwrap_or(&project.root_message_id),
                    created,
                )
                .await
            }
            _ => Ok(()),
        }
    }
    pub(in crate::control) async fn git_baseline(
        &self,
        workspace: &dyn ProjectWorkspace,
        command: &Command,
    ) -> Result<Option<GitBaseline>, ApiError> {
        match self {
            Self::Conversation(tx) => {
                crate::control::project::git::command_git_baseline(workspace, &tx.original, command)
                    .await
            }
            Self::CronTrigger(tx) => {
                crate::control::project::git::cron_git_baseline(workspace, &tx.original, command)
                    .await
            }
            _ => Ok(None),
        }
    }
    /// Record references selecting filesystem work must remain stable on CAS retry.
    /// Project Git identity is checked separately through `PreparedProject::verify`.
    pub(in crate::control) fn preparation_key(&self) -> PreparationKey {
        match self {
            Self::NewSession(tx) => session_preparation_key(&tx.original),
            Self::Conversation(tx) => session_preparation_key(&tx.original),
            Self::CronTrigger(tx) => PreparationKey::Cron {
                projects: tx.original.projects.clone(),
                crons: tx.original.crons.clone(),
            },
            _ => PreparationKey::None,
        }
    }
    pub(in crate::control) fn read(self, command: Command) -> Result<CommandResult, ApiError> {
        match (self, command) {
            (Self::Runs(loaded), Command::GetRun { run_id }) => loaded
                .original
                .runs
                .into_iter()
                .find(|run| run.id == run_id)
                .map(|run| CommandResult::Run(run.view()))
                .ok_or_else(|| error(ErrorCode::InvalidRun, "run not found", false)),
            (Self::Archive(loaded), Command::ExportProject { project_id }) => {
                export_project(&loaded.original, loaded.revision, &project_id)
                    .map(CommandResult::ProjectExport)
            }
            (Self::Settings(loaded), Command::GetSettings) => {
                Ok(CommandResult::Settings(settings_view(&loaded.original)))
            }
            (Self::Projects(loaded), Command::ListProjects) => Ok(CommandResult::Projects(
                loaded
                    .original
                    .projects
                    .iter()
                    .map(crate::control::model::ProjectState::view)
                    .collect(),
            )),
            (Self::Agents(loaded), Command::ListAgents) => Ok(CommandResult::Agents(
                loaded
                    .original
                    .agents
                    .iter()
                    .map(crate::control::model::AgentState::view)
                    .collect(),
            )),
            (Self::Provider(loaded), Command::ListAgentProviders) => {
                Ok(CommandResult::AgentProviders(
                    loaded
                        .original
                        .providers
                        .iter()
                        .map(crate::control::model::ProviderState::view)
                        .collect(),
                ))
            }
            (Self::Sessions(loaded), Command::ListSessions { .. }) => Ok(CommandResult::Sessions(
                loaded
                    .original
                    .sessions
                    .iter()
                    .map(crate::control::model::SessionState::view)
                    .collect(),
            )),
            (Self::Messages(loaded), Command::ListMessages { .. }) => Ok(CommandResult::Messages(
                loaded
                    .original
                    .messages
                    .iter()
                    .map(crate::control::model::MessageState::view)
                    .collect(),
            )),
            (Self::Runs(loaded), Command::ListRuns { .. }) => Ok(CommandResult::Runs(
                loaded
                    .original
                    .runs
                    .iter()
                    .map(crate::control::model::RunState::view)
                    .collect(),
            )),
            (Self::Crons(loaded), Command::ListCrons) => Ok(CommandResult::Crons(
                loaded
                    .original
                    .crons
                    .iter()
                    .map(crate::control::model::CronState::view)
                    .collect(),
            )),
            _ => unreachable!("read command/context mismatch"),
        }
    }
    #[allow(clippy::too_many_lines)]
    pub(in crate::control) async fn reduce(
        &self,
        workspace: &dyn ProjectWorkspace,
        command: Command,
        user_git_baseline: Option<&GitBaseline>,
        permission_limits: PermissionPolicyLimits,
        derive_source_locked: bool,
        prepared_project: Option<&crate::control::project::git::PreparedProject>,
    ) -> Result<CommandCommit, ApiError> {
        macro_rules! reduce {
            ($loaded:ident, $state:ident, $expr:expr) => {{
                let mut $state = $loaded.original.clone();
                let (outcome, events) = $expr?;
                Ok(CommandCommit {
                    revision: $loaded.revision,
                    changes: $loaded.changes(&$state).map_err(store_error)?,
                    events,
                    outcome,
                })
            }};
        }
        macro_rules! ready {
            ($expr:expr) => {
                $expr.map(|(result, events)| (CommandOutcome::Ready(Box::new(result)), events))
            };
        }
        match (self, command) {
            (
                Self::ProjectRegistration(loaded),
                Command::RegisterProject {
                    id,
                    name,
                    workdir: _,
                    repo_url,
                },
            ) => reduce!(
                loaded,
                state,
                ready!(register_project(
                    &mut state,
                    id,
                    name,
                    prepared_project.expect("Project prepared before reduction"),
                    repo_url,
                ))
            ),
            (
                Self::ProjectAgent(loaded),
                Command::SetProjectDefaultAgent {
                    project_id,
                    agent_id,
                },
            ) => reduce!(
                loaded,
                state,
                ready!(set_project_default_agent(
                    &mut state,
                    &project_id,
                    &agent_id
                ))
            ),
            (
                Self::ProjectAgent(loaded),
                Command::UpdateProject {
                    project_id,
                    name,
                    agent_id,
                },
            ) => reduce!(
                loaded,
                state,
                ready!(update_project(
                    &mut state,
                    &project_id,
                    &name,
                    agent_id.as_deref()
                ))
            ),
            (Self::Agent(loaded), Command::RegisterAgent { id, name, config }) => reduce!(
                loaded,
                state,
                ready!(register_agent(&mut state, id, name, config))
            ),
            (Self::Agent(loaded), Command::UpdateAgent { id, name, config }) => reduce!(
                loaded,
                state,
                ready!(update_agent(&mut state, &id, name, config))
            ),
            (Self::SessionConfig(loaded), Command::SetSessionConfig { session_id, config }) => {
                reduce!(
                    loaded,
                    state,
                    ready!({ set_session_config(&mut state, &session_id, config) })
                )
            }
            (
                Self::NewSession(loaded),
                Command::CreateSession {
                    id,
                    project_id,
                    agent_id,
                    at_message_id,
                },
            ) => reduce!(
                loaded,
                state,
                ready!(create_session(
                    &mut state,
                    id,
                    project_id,
                    &agent_id,
                    at_message_id
                ))
            ),
            (
                Self::SessionBinding(loaded),
                Command::SetSessionAgent {
                    session_id,
                    agent_id,
                },
            ) => reduce!(
                loaded,
                state,
                ready!(set_session_agent(&mut state, &session_id, &agent_id))
            ),
            (Self::Sessions(loaded), Command::RenameSession { session_id, name }) => reduce!(
                loaded,
                state,
                ready!(rename_session(&mut state, &session_id, &name))
            ),
            (Self::Sessions(loaded), Command::SetSessionTitle { session_id, title }) => reduce!(
                loaded,
                state,
                ready!({ set_session_title(&mut state, &session_id, &title) })
            ),
            (Self::Conversation(loaded), Command::SendMessage { session_id, text }) => {
                reduce!(loaded, state, {
                    send_message(
                        &mut state,
                        session_id,
                        text,
                        require_user_git_baseline(user_git_baseline)?,
                        permission_limits,
                    )
                })
            }
            (
                Self::Conversation(loaded),
                Command::ForkSession {
                    id,
                    project_id,
                    agent_id,
                    at_message_id,
                    text,
                },
            ) => reduce!(loaded, state, {
                fork_session(
                    &mut state,
                    ForkSessionInput {
                        id,
                        project_id,
                        agent_id,
                        at_message_id,
                        text,
                    },
                    require_user_git_baseline(user_git_baseline)?,
                    permission_limits,
                )
            }),
            (
                Self::Conversation(loaded),
                Command::DeriveSession {
                    id,
                    project_id,
                    source_session_id,
                    agent_id,
                    at_message_id,
                    text,
                },
            ) => reduce!(loaded, state, {
                derive_session(
                    &mut state,
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
                )
            }),
            (Self::RunControl(loaded), Command::CancelRun { run_id }) => {
                reduce!(loaded, state, ready!(cancel_run(&mut state, &run_id)))
            }
            (
                Self::RunControl(loaded),
                Command::ResolveNativeApproval {
                    run_id,
                    approval_id,
                    action,
                    scope,
                },
            ) => reduce!(
                loaded,
                state,
                ready!(
                    resolve_native_approval(
                        workspace,
                        &mut state,
                        &run_id,
                        &approval_id,
                        action,
                        scope,
                        permission_limits,
                    )
                    .await
                )
            ),
            (
                Self::CronCreate(loaded),
                Command::CreateCron {
                    id,
                    name,
                    project_id,
                    base_message_id,
                    agent_id,
                    schedule,
                    timezone,
                },
            ) => reduce!(
                loaded,
                state,
                ready!(create_cron(
                    &mut state,
                    id,
                    name,
                    project_id,
                    base_message_id,
                    &agent_id,
                    schedule,
                    timezone,
                ))
            ),
            (Self::Crons(loaded), Command::SetCronEnabled { cron_id, enabled }) => reduce!(
                loaded,
                state,
                ready!(set_cron_enabled(&mut state, &cron_id, enabled))
            ),
            (
                Self::CronTrigger(loaded),
                Command::TriggerCron {
                    cron_id,
                    scheduled_at,
                },
            ) => reduce!(loaded, state, {
                trigger_cron(
                    &mut state,
                    &cron_id,
                    scheduled_at,
                    user_git_baseline,
                    permission_limits,
                )
            }),
            (
                Self::Archive(loaded),
                Command::ImportProject {
                    archive,
                    workdir: _,
                },
            ) => reduce!(
                loaded,
                state,
                ready!(import_project(
                    &mut state,
                    archive,
                    prepared_project.expect("Project prepared before reduction")
                ))
            ),
            (
                Self::Settings(loaded),
                Command::SaveSettings {
                    expected_revision,
                    values,
                },
            ) => reduce!(
                loaded,
                state,
                ready!(save_settings(&mut state, expected_revision, values))
            ),
            (Self::Settings(loaded), Command::ResetSettings) => {
                reduce!(loaded, state, ready!(Ok(reset_settings(&mut state))))
            }
            _ => unreachable!("mutating command/context mismatch"),
        }
    }
}

#[derive(PartialEq)]
pub(in crate::control) enum PreparationKey {
    None,
    Session {
        projects: Vec<ProjectState>,
        sessions: Vec<SessionState>,
        messages: Vec<MessageBaseline>,
    },
    Cron {
        projects: Vec<ProjectState>,
        crons: Vec<CronState>,
    },
}
#[derive(PartialEq)]
pub(in crate::control) struct MessageBaseline {
    id: String,
    parent: Option<String>,
    git_commit: Option<String>,
    workspace_commit: Option<String>,
}
fn session_preparation_key(
    state: &(
         impl crate::control::state::HasProjects
         + crate::control::state::HasSessions
         + crate::control::state::HasMessages
     ),
) -> PreparationKey {
    PreparationKey::Session {
        projects: state.projects().clone(),
        sessions: state.sessions().clone(),
        messages: state
            .messages()
            .iter()
            .map(|m| MessageBaseline {
                id: m.id.clone(),
                parent: m.parent_message_id.clone(),
                git_commit: m.git_commit.clone(),
                workspace_commit: m
                    .data
                    .as_ref()
                    .and_then(|d| d.pointer("/codex/commit_id"))
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned),
            })
            .collect(),
    }
}
