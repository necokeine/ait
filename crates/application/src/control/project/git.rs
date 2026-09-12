//! Project Git initialization and immutable HEAD/index baselines.
use crate::control::catalog::{require_agent, validate_config};
use crate::control::conversation::derive_reuses_source;
use crate::control::errors::error;
use crate::control::project::worktrees::session_worktree_path;
use crate::control::state::WorkingSet;
use ait_contracts::{AgentMode, ApiError, Command};
use ait_domain::ErrorCode;
use std::path::{Path, PathBuf};
use std::process::Command as ProcessCommand;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::control) struct GitBaseline {
    pub(in crate::control) commit: String,
    pub(in crate::control) index_tree: String,
}

pub(in crate::control) fn absolute_git_dir(workdir: &Path) -> Result<PathBuf, ApiError> {
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

pub(in crate::control) fn git_symbolic_head(workdir: &Path) -> Result<Option<String>, ApiError> {
    let output = ProcessCommand::new("git")
        .arg("-C")
        .arg(workdir)
        .args(["symbolic-ref", "--quiet", "HEAD"])
        .output()
        .map_err(|failure| {
            error(
                ErrorCode::ProjectGitHeadUnavailable,
                format!("cannot inspect Project branch identity: {failure}"),
                false,
            )
        })?;
    if output.status.success() {
        return Ok(Some(
            String::from_utf8_lossy(&output.stdout).trim().to_owned(),
        ));
    }
    if output.status.code() == Some(1) {
        Ok(None)
    } else {
        Err(error(
            ErrorCode::ProjectGitHeadUnavailable,
            String::from_utf8_lossy(&output.stderr).trim(),
            false,
        ))
    }
}

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

pub(in crate::control) fn prepare_git_root(path: &Path) -> Result<std::path::PathBuf, ApiError> {
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

pub(in crate::control) fn ensure_git_head(path: &Path) -> Result<String, ApiError> {
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

pub(in crate::control) fn git_stdout(cwd: &Path, arguments: &[&str]) -> Result<String, ApiError> {
    let output = ProcessCommand::new("git")
        .arg("-C")
        .arg(cwd)
        .args(arguments)
        .output()
        .map_err(|failure| {
            error(
                ErrorCode::ProjectGitHeadUnavailable,
                format!("cannot run Git for Session worktree: {failure}"),
                false,
            )
        })?;
    if !output.status.success() {
        return Err(error(
            ErrorCode::ProjectGitInitFailed,
            String::from_utf8_lossy(&output.stderr).trim(),
            false,
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

pub(in crate::control) fn command_git_baseline(
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
    clean_git_baseline(&path).map(Some)
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

pub(in crate::control) fn git_head(path: &Path) -> Result<Option<String>, ApiError> {
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

pub(in crate::control) fn is_git_commit(value: &str) -> bool {
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
