//! Project Git initialization and immutable HEAD/index baselines.
use crate::control::catalog::{require_agent, validate_config};
use crate::control::conversation::derive_reuses_source;
use crate::control::errors::error;
use crate::control::errors::project_error;
use crate::control::persistence::{
    HasAgents, HasCrons, HasMessages, HasProjects, HasProviders, HasRuns, HasSessions, HasSettings,
};
use crate::control::project::worktrees::session_worktree_path;
use crate::control::settings::resolve_project_agent_id;
use ait_contracts::{AgentMode, ApiError, Command};
use ait_domain::{CodexWorkspaceMode, ErrorCode, SessionSource};
use ait_workspace::ProjectWorkspace;
use std::path::{Path, PathBuf};

pub(in crate::control) use ait_workspace::GitBaseline;

pub(in crate::control) fn require_user_git_baseline(
    baseline: Option<&GitBaseline>,
) -> Result<&GitBaseline, ApiError> {
    baseline.ok_or_else(|| {
        error(
            ErrorCode::ProjectGitHeadUnavailable,
            "Git HEAD/index snapshot is missing for user message",
            false,
        )
    })
}

/// Resolve the target before the canonical uniqueness read, without changing it.
pub(in crate::control) async fn canonical_project_path(
    workspace: &dyn ProjectWorkspace,
    path: &Path,
) -> Result<PathBuf, ApiError> {
    workspace
        .path_facts(path, path)
        .await
        .map(|facts| facts.canonical_root)
        .map_err(project_error)
}

pub(in crate::control) async fn command_git_baseline(
    workspace: &dyn ProjectWorkspace,
    state: &(impl HasMessages + HasProjects + HasSessions + HasSettings),
    command: &Command,
) -> Result<Option<GitBaseline>, ApiError> {
    let path = match command {
        Command::SendMessage { session_id, .. } => {
            let session = state
                .sessions()
                .iter()
                .find(|session| session.id == *session_id)
                .ok_or_else(|| error(ErrorCode::SessionNotFound, "session not found", false))?;
            if matches!(
                &session.source,
                SessionSource::CodexThread(source)
                    if matches!(source.workspace_mode, CodexWorkspaceMode::NativeCwd { .. })
            ) {
                return Ok(None);
            }
            Some(PathBuf::from(&session.workdir))
        }
        Command::ForkSession { id, project_id, .. } => {
            let project = state
                .projects()
                .iter()
                .find(|project| project.id == *project_id)
                .ok_or_else(|| error(ErrorCode::InvalidProject, "project not found", false))?;
            Some(session_worktree_path(&project.workdir, id)?)
        }
        Command::DeriveSession {
            id,
            project_id,
            source_session_id,
            agent_id,
            at_message_id,
            ..
        } => {
            let agent_id = resolve_project_agent_id(state, project_id, agent_id)?;
            let source = state
                .sessions()
                .iter()
                .find(|session| session.id == *source_session_id)
                .ok_or_else(|| error(ErrorCode::SessionNotFound, "session not found", false))?;
            if derive_reuses_source(state, id, project_id, source, &agent_id, at_message_id) {
                Some(PathBuf::from(&source.workdir))
            } else {
                let project = state
                    .projects()
                    .iter()
                    .find(|project| project.id == *project_id)
                    .ok_or_else(|| error(ErrorCode::InvalidProject, "project not found", false))?;
                Some(session_worktree_path(&project.workdir, id)?)
            }
        }
        _ => return Ok(None),
    };
    let Some(path) = path else {
        return Ok(None);
    };
    workspace
        .clean_baseline(&path)
        .await
        .map(Some)
        .map_err(project_error)
}

pub(in crate::control) fn is_git_commit(value: &str) -> bool {
    matches!(value.len(), 40 | 64)
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

pub(in crate::control) async fn cron_git_baseline(
    workspace: &dyn ProjectWorkspace,
    state: &(impl HasAgents + HasCrons + HasProjects + HasProviders + HasRuns),
    command: &Command,
) -> Result<Option<GitBaseline>, ApiError> {
    let path = match command {
        Command::TriggerCron {
            cron_id,
            scheduled_at,
        } => {
            if state.runs().iter().any(|run| {
                run.cron_id.as_deref() == Some(cron_id.as_str())
                    && run.scheduled_at == Some(*scheduled_at)
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
            if validate_config(state, &agent.config)?.kind != AgentMode::Codex {
                return Ok(None);
            }
            let project = state
                .projects()
                .iter()
                .find(|project| project.id == cron.project_id)
                .ok_or_else(|| error(ErrorCode::InvalidProject, "project not found", false))?;
            Some(session_worktree_path(
                &project.workdir,
                &crate::control::cron::cron_session_id(cron_id, *scheduled_at),
            )?)
        }
        _ => return Ok(None),
    };
    match path {
        Some(path) => workspace
            .clean_baseline(&path)
            .await
            .map(Some)
            .map_err(project_error),
        None => Ok(None),
    }
}

#[derive(Clone)]
pub(in crate::control) struct PreparedProject {
    pub(in crate::control) workdir: String,
    pub(in crate::control) base_commit: String,
}
impl PreparedProject {
    /// Revalidate the frozen preparation before each CAS, without repairing Git.
    pub(in crate::control) async fn verify(
        &self,
        workspace: &dyn ProjectWorkspace,
    ) -> Result<(), ApiError> {
        let expected = Path::new(&self.workdir);
        if workspace.verify_git_root(expected).await.is_err()
            || workspace.git_head(expected).await.ok().flatten().as_deref()
                != Some(self.base_commit.as_str())
        {
            return Err(error(
                ErrorCode::RunQueueConflict,
                "prepared Project Git root or HEAD changed; retry the request",
                true,
            ));
        }
        Ok(())
    }
}
pub(in crate::control) async fn prepare_project(
    workspace: &dyn ProjectWorkspace,
    workdir: &str,
) -> Result<PreparedProject, ApiError> {
    let expected = Path::new(workdir);
    let canonical = workspace
        .prepare_git_root(expected, Some(expected))
        .await
        .map_err(project_error)?;
    Ok(PreparedProject {
        base_commit: workspace
            .ensure_git_head(&canonical)
            .await
            .map_err(project_error)?,
        workdir: canonical.to_string_lossy().into_owned(),
    })
}
