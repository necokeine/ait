//! Project Git initialization and immutable HEAD/index baselines.
use crate::control::catalog::{require_agent, validate_config};
use crate::control::conversation::derive_reuses_source;
use crate::control::errors::error;
use crate::control::project::worktrees::session_worktree_path;
use crate::control::state::WorkingSet;
use ait_contracts::{AgentMode, ApiError, Command};
use ait_domain::ErrorCode;
pub(in crate::control) use ait_ports::GitBaseline;
use ait_ports::ProjectWorkspace;
use std::path::PathBuf;

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

pub(in crate::control) async fn command_git_baseline(
    workspace: &dyn ProjectWorkspace,
    state: &WorkingSet,
    command: &Command,
) -> Result<Option<GitBaseline>, ApiError> {
    let path = match command {
        Command::SendMessage { session_id, .. } => Some(PathBuf::from(
            &state
                .sessions
                .iter()
                .find(|session| session.id == *session_id)
                .ok_or_else(|| error(ErrorCode::SessionNotFound, "session not found", false))?
                .workdir,
        )),
        Command::ForkSession { id, project_id, .. } => {
            let project = state
                .projects
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
            let source = state
                .sessions
                .iter()
                .find(|session| session.id == *source_session_id)
                .ok_or_else(|| error(ErrorCode::SessionNotFound, "session not found", false))?;
            if derive_reuses_source(state, id, project_id, source, agent_id, at_message_id) {
                Some(PathBuf::from(&source.workdir))
            } else {
                let project = state
                    .projects
                    .iter()
                    .find(|project| project.id == *project_id)
                    .ok_or_else(|| error(ErrorCode::InvalidProject, "project not found", false))?;
                Some(session_worktree_path(&project.workdir, id)?)
            }
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
            let project = state
                .projects
                .iter()
                .find(|project| project.id == cron.project_id)
                .ok_or_else(|| error(ErrorCode::InvalidProject, "project not found", false))?;
            Some(PathBuf::from(&project.workdir))
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
        .map_err(crate::control::errors::project_error)
}

pub(in crate::control) fn is_git_commit(value: &str) -> bool {
    matches!(value.len(), 40 | 64)
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}
