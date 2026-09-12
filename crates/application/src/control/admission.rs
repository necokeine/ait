//! Session exclusion and canonical Project workspace write leases.
use crate::control::LocalControlService;
use crate::control::catalog::{require_agent, validate_config};
use crate::control::errors::error;
use crate::control::permissions::{PermissionPolicyLimits, effective_permission_profile};
use crate::control::project::git::absolute_git_dir;
use crate::control::project::require_project_view;
use crate::control::state::WorkingSet;
use ait_contracts::{AgentMode, ApiError, Command, SessionView};
use ait_domain::ErrorCode;
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Weak};

/// Owns both the async in-process queue position and the process-wide advisory
/// lock for one canonical Project Git worktree.
pub(in crate::control) struct WorkspaceWriteLease {
    _process_guard: tokio::sync::OwnedMutexGuard<()>,
    _file: File,
}

pub(in crate::control) fn workspace_write_path(
    state: &WorkingSet,
    command: &Command,
    permission_limits: PermissionPolicyLimits,
) -> Result<Option<PathBuf>, ApiError> {
    if let Some(project_id) = session_command_project_id(state, command)? {
        let project = require_project_view(state, project_id)?;
        return Ok(Some(PathBuf::from(&project.workdir)));
    }
    let Command::TriggerCron {
        cron_id,
        scheduled_at,
    } = command
    else {
        return Ok(None);
    };
    if state.runs.iter().any(|run| {
        run.cron_id.as_deref() == Some(cron_id.as_str()) && run.scheduled_at == Some(*scheduled_at)
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
    let provider = validate_config(state, &agent.config)?;
    let _ = effective_permission_profile(&state.settings, provider, permission_limits)?;
    if !matches!(
        provider.kind,
        AgentMode::Codex | AgentMode::OpenAI | AgentMode::DeepSeek
    ) {
        return Ok(None);
    }
    let project = require_project_view(state, &cron.project_id)?;
    Ok(Some(PathBuf::from(&project.workdir)))
}

fn session_command_project_id<'a>(
    state: &'a WorkingSet,
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
                .sessions
                .iter()
                .find(|session| session.id == *source_session_id)
                .ok_or_else(|| error(ErrorCode::SessionNotFound, "session not found", false))?;
            Ok(Some(project_id))
        }
        Command::SendMessage { session_id, .. } => state
            .sessions
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

pub(in crate::control) fn ensure_idle(session: &SessionView) -> Result<(), ApiError> {
    if session.active_run_id.is_some() {
        Err(busy())
    } else {
        Ok(())
    }
}

pub(in crate::control) fn check_session_admission(
    state: &WorkingSet,
    command: &Command,
) -> Result<(), ApiError> {
    if let Some(id) = command_session(command)
        && let Some(session) = state.sessions.iter().find(|s| s.id == id)
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
        let state = self.read_command_records(command).await?.original;
        check_session_admission(&state, command)?;
        let Some(workdir) = workspace_write_path(&state, command, self.permission_limits)? else {
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
            Ok(()) => Ok(WorkspaceWriteLease {
                _process_guard: process_guard,
                _file: file,
            }),
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
