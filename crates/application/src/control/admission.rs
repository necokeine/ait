//! Session exclusion and canonical Project workspace write leases.
use crate::control::LocalControlService;
use crate::control::catalog::{require_agent, validate_config};
use crate::control::conversation::SessionRecord;
use crate::control::errors::error;
use crate::control::permissions::{PermissionPolicyLimits, effective_permission_profile};
use crate::control::persistence::{
    HasAgents, HasCrons, HasProjects, HasProviders, HasRuns, HasSessions, HasSettings,
};
use crate::control::project::require_project_view;
use ait_contracts::{AgentMode, ApiError, Command};
use ait_domain::ErrorCode;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Weak};

pub(in crate::control) type WorkspaceWriteLease = Arc<dyn ait_workspace::WorkspaceLease>;

pub(in crate::control) fn workspace_write_path(
    state: &(impl HasProjects + HasSessions),
    command: &Command,
    _permission_limits: PermissionPolicyLimits,
) -> Result<Option<PathBuf>, ApiError> {
    if let Some(project_id) = session_command_project_id(state, command)? {
        let project = require_project_view(state, project_id)?;
        return Ok(Some(PathBuf::from(&project.workdir)));
    }
    Ok(None)
}

pub(in crate::control) fn cron_workspace_write_path(
    state: &(impl HasAgents + HasCrons + HasProjects + HasProviders + HasRuns + HasSettings),
    command: &Command,
    permission_limits: PermissionPolicyLimits,
) -> Result<Option<PathBuf>, ApiError> {
    let Command::TriggerCron {
        cron_id,
        scheduled_at,
    } = command
    else {
        return Ok(None);
    };
    if state.runs().iter().any(|run| {
        run.cron_id.as_deref() == Some(cron_id.as_str()) && run.scheduled_at == Some(*scheduled_at)
    }) {
        return Ok(None);
    }
    let Some(cron) = state
        .crons()
        .iter()
        .find(|cron| cron.id == *cron_id && cron.enabled)
    else {
        return Ok(None);
    };
    let agent = require_agent(state, &cron.agent_id)?;
    let provider = validate_config(state, &agent.config)?;
    let _ = effective_permission_profile(state.settings(), provider, permission_limits)?;
    if !matches!(
        provider.kind,
        AgentMode::Codex
            | AgentMode::OpenAI
            | AgentMode::DeepSeek
            | AgentMode::Gemini
            | AgentMode::MiniMax
    ) {
        return Ok(None);
    }
    let project = require_project_view(state, &cron.project_id)?;
    Ok(Some(PathBuf::from(&project.workdir)))
}

fn session_command_project_id<'a>(
    state: &'a impl HasSessions,
    command: &'a Command,
) -> Result<Option<&'a str>, ApiError> {
    match command {
        Command::CreateSession { project_id, .. } | Command::ForkSession { project_id, .. } => {
            Ok(Some(project_id))
        }
        Command::DeriveSession {
            project_id,
            source_session_id,
            ..
        } => {
            state
                .sessions()
                .iter()
                .find(|session| session.id == *source_session_id)
                .ok_or_else(|| error(ErrorCode::SessionNotFound, "session not found", false))?;
            Ok(Some(project_id))
        }
        Command::SendMessage { session_id, .. } => state
            .sessions()
            .iter()
            .find(|session| session.id == *session_id)
            .map(|session| Some(session.project_id.as_str()))
            .ok_or_else(|| error(ErrorCode::SessionNotFound, "session not found", false)),
        _ => Ok(None),
    }
}

fn command_session(command: &Command) -> Option<&str> {
    match command {
        Command::SendMessage { session_id, .. }
        | Command::SetSessionAgent { session_id, .. }
        | Command::SetSessionConfig { session_id, .. } => Some(session_id),
        Command::ForkSession { id, .. }
        | Command::DeriveSession { id, .. }
        | Command::CreateSession { id, .. } => Some(id),
        _ => None,
    }
}

pub(in crate::control) struct SessionAdmission {
    leases: Vec<(String, Arc<()>)>,
    derive_source_locked: bool,
}

impl SessionAdmission {
    pub(in crate::control) const fn derive_source_locked(&self) -> bool {
        self.derive_source_locked
    }

    pub(in crate::control) fn retain_for_session(&mut self, session_id: &str) {
        self.leases.retain(|(id, _)| id == session_id);
    }
}

fn busy() -> ApiError {
    error(
        ErrorCode::SessionBusy,
        "session already has an active run or operation",
        false,
    )
}

pub(in crate::control) fn ensure_idle(session: &SessionRecord) -> Result<(), ApiError> {
    if session.active_run_id().is_some() {
        Err(busy())
    } else {
        Ok(())
    }
}

pub(in crate::control) fn check_session_admission(
    state: &impl HasSessions,
    command: &Command,
) -> Result<(), ApiError> {
    if let Some(id) = command_session(command)
        && let Some(session) = state.sessions().iter().find(|s| s.id == id)
    {
        ensure_idle(session)?;
    }
    Ok(())
}

impl LocalControlService {
    pub(in crate::control) async fn acquire_workspace_write(
        &self,
        command: &Command,
    ) -> Result<Option<WorkspaceWriteLease>, ApiError> {
        let state = self.read_command_records(command).await?;
        state.check_admission(command)?;
        let Some(workdir) = state.workspace_path(command, self.permission_limits)? else {
            return Ok(None);
        };
        self.acquire_workspace_path(&workdir).await.map(Some)
    }

    pub(in crate::control) async fn acquire_workspace_write_for_run(
        &self,
        run_id: &str,
    ) -> Result<WorkspaceWriteLease, ApiError> {
        let state = self.read_run_records(run_id).await?.original;
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
        self.acquire_workspace_path(Path::new(&project.workdir))
            .await
    }

    async fn acquire_workspace_path(
        &self,
        workdir: &Path,
    ) -> Result<WorkspaceWriteLease, ApiError> {
        self.project_workspace
            .acquire_lease(workdir)
            .await
            .map_err(crate::control::errors::project_error)
    }

    pub(in crate::control) fn acquire_session(
        &self,
        command: &Command,
    ) -> Result<SessionAdmission, ApiError> {
        let Some(id) = command_session(command) else {
            return Ok(SessionAdmission {
                leases: Vec::new(),
                derive_source_locked: false,
            });
        };
        let mut leases = self.session_leases.lock().map_err(|_| busy())?;
        leases.retain(|_, lease| lease.strong_count() > 0);
        if leases.get(id).and_then(Weak::upgrade).is_some() {
            return Err(busy());
        }
        let lease = Arc::new(());
        leases.insert(id.into(), Arc::downgrade(&lease));
        let mut owned = vec![(id.to_owned(), lease)];
        let derive_source_locked = if let Command::DeriveSession {
            source_session_id, ..
        } = command
        {
            if source_session_id == id
                || leases
                    .get(source_session_id)
                    .and_then(Weak::upgrade)
                    .is_some()
            {
                false
            } else {
                let source_lease = Arc::new(());
                leases.insert(source_session_id.clone(), Arc::downgrade(&source_lease));
                owned.push((source_session_id.clone(), source_lease));
                true
            }
        } else {
            false
        };
        Ok(SessionAdmission {
            leases: owned,
            derive_source_locked,
        })
    }
}
