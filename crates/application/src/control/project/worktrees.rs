//! Manager-owned Session worktree preparation and validation.
use crate::control::catalog::require_agent;
use crate::control::conversation::derive_reuses_source;
use crate::control::conversation::messages::{
    message_workspace_commit, validate_message_text, validate_session_message,
};
use crate::control::errors::error;
use crate::control::errors::project_error;
use crate::control::project::archive::{validate_import_conflicts, validate_project_export};
use crate::control::project::require_project_view;
use crate::control::state::WorkingSet;
use ait_contracts::{ApiError, Command, ProjectExport, ProjectView, RunView};
use ait_domain::ErrorCode;
use ait_ports::{ProjectWorkspace, WorkspaceLease};
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub(in crate::control) fn validate_session_path_component(id: &str) -> Result<(), ApiError> {
    // Session worktrees share .ait with the Project database and its sidecars.
    if [
        "project.sqlite3",
        "project.sqlite3-wal",
        "project.sqlite3-shm",
        "project.sqlite3-journal",
    ]
    .iter()
    .any(|reserved| {
        id.trim_end_matches([' ', '.'])
            .eq_ignore_ascii_case(reserved)
    }) {
        return Err(error(
            ErrorCode::InvalidSession,
            "session id is reserved for project storage",
            false,
        ));
    }
    if id.trim().is_empty()
        || id == "."
        || id == ".."
        || id.contains('/')
        || id.contains('\\')
        || id.chars().any(char::is_control)
    {
        return Err(error(
            ErrorCode::InvalidSession,
            "session id must be a nonempty safe path component",
            false,
        ));
    }
    Ok(())
}

pub(in crate::control) fn session_worktree_path(
    project_workdir: &str,
    session_id: &str,
) -> Result<PathBuf, ApiError> {
    validate_session_path_component(session_id)?;
    Ok(Path::new(project_workdir).join(".ait").join(session_id))
}

pub(in crate::control) fn run_workdir(
    state: &WorkingSet,
    run: &RunView,
) -> Result<PathBuf, ApiError> {
    let project = state
        .projects
        .iter()
        .find(|project| project.id == run.project_id)
        .ok_or_else(|| error(ErrorCode::InvalidProject, "project not found", false))?;
    let Some(session_id) = run.session_id.as_deref() else {
        return Ok(PathBuf::from(&project.workdir));
    };
    let session = state
        .sessions
        .iter()
        .find(|session| session.id == session_id && session.project_id == run.project_id)
        .ok_or_else(|| error(ErrorCode::SessionNotFound, "run Session not found", false))?;
    let expected = session_worktree_path(&project.workdir, session_id)?;
    if Path::new(&session.workdir) != expected {
        return Err(error(
            ErrorCode::InvalidSession,
            "Session workdir is outside its manager-owned worktree path",
            false,
        ));
    }
    Ok(expected)
}

pub(in crate::control) async fn prepare_command_session_worktrees(
    workspace: &dyn ProjectWorkspace,
    lease: Option<Arc<dyn WorkspaceLease>>,
    state: &WorkingSet,
    command: &Command,
    created: &mut Vec<PathBuf>,
) -> Result<(), ApiError> {
    match command {
        Command::ImportProject { archive, workdir } => {
            prepare_import_session_worktrees(
                workspace,
                lease.clone(),
                state,
                archive,
                workdir,
                created,
            )
            .await
        }
        Command::CreateSession {
            id,
            project_id,
            agent_id,
            at_message_id,
        } => {
            let project = require_project_view(state, project_id)?;
            let message_id = at_message_id
                .as_deref()
                .unwrap_or(project.root_message_id.as_str());
            prepare_new_session_worktree(
                workspace,
                lease.clone(),
                state,
                id,
                project_id,
                agent_id,
                message_id,
                created,
            )
            .await
        }
        Command::ForkSession {
            id,
            project_id,
            agent_id,
            at_message_id,
            text,
        } => {
            validate_message_text(text)?;
            prepare_new_session_worktree(
                workspace,
                lease.clone(),
                state,
                id,
                project_id,
                agent_id,
                at_message_id,
                created,
            )
            .await
        }
        Command::DeriveSession {
            id,
            project_id,
            source_session_id,
            agent_id,
            at_message_id,
            text,
        } => {
            prepare_derived_session_worktree(
                workspace,
                lease.clone(),
                state,
                id,
                project_id,
                source_session_id,
                agent_id,
                at_message_id,
                text,
                created,
            )
            .await
        }
        Command::SendMessage { session_id, .. } => {
            prepare_existing_session_worktree(workspace, lease.clone(), state, session_id, created)
                .await
        }
        _ => Ok(()),
    }
}

async fn prepare_import_session_worktrees(
    workspace: &dyn ProjectWorkspace,
    lease: Option<Arc<dyn WorkspaceLease>>,
    state: &WorkingSet,
    archive: &ProjectExport,
    workdir: &str,
    created: &mut Vec<PathBuf>,
) -> Result<(), ApiError> {
    validate_project_export(archive)?;
    validate_import_conflicts(state, archive)?;
    let canonical = workspace
        .prepare_git_root(Path::new(workdir))
        .await
        .map_err(project_error)?;
    let mut project = archive.project.clone();
    project.workdir = canonical.to_string_lossy().into_owned();
    project.base_commit = workspace
        .ensure_git_head(&canonical)
        .await
        .map_err(project_error)?;
    for session in &archive.sessions {
        ensure_session_worktree(
            workspace,
            lease.clone(),
            &project,
            &session.id,
            &project.base_commit,
            created,
        )
        .await?;
    }
    Ok(())
}

#[allow(
    clippy::too_many_arguments,
    reason = "Preserve Session selection inputs alongside the workspace lease"
)]
async fn prepare_new_session_worktree(
    workspace: &dyn ProjectWorkspace,
    lease: Option<Arc<dyn WorkspaceLease>>,
    state: &WorkingSet,
    id: &str,
    project_id: &str,
    agent_id: &str,
    message_id: &str,
    created: &mut Vec<PathBuf>,
) -> Result<(), ApiError> {
    validate_session_path_component(id)?;
    if state.sessions.iter().any(|session| session.id == id) {
        return Err(error(
            ErrorCode::InvalidSession,
            "session id is already registered",
            false,
        ));
    }
    require_agent(state, agent_id)?;
    let project = require_project_view(state, project_id)?;
    validate_session_message(state, project_id, message_id)?;
    let baseline = message_workspace_commit(state, project, message_id)?;
    ensure_session_worktree(workspace, lease.clone(), project, id, &baseline, created).await
}

#[allow(clippy::too_many_arguments)]
async fn prepare_derived_session_worktree(
    workspace: &dyn ProjectWorkspace,
    lease: Option<Arc<dyn WorkspaceLease>>,
    state: &WorkingSet,
    id: &str,
    project_id: &str,
    source_session_id: &str,
    agent_id: &str,
    at_message_id: &str,
    text: &str,
    created: &mut Vec<PathBuf>,
) -> Result<(), ApiError> {
    validate_message_text(text)?;
    validate_session_path_component(id)?;
    let source = state
        .sessions
        .iter()
        .find(|session| session.id == source_session_id)
        .ok_or_else(|| error(ErrorCode::SessionNotFound, "session not found", false))?;
    if !derive_reuses_source(state, id, project_id, source, agent_id, at_message_id) {
        return prepare_new_session_worktree(
            workspace,
            lease.clone(),
            state,
            id,
            project_id,
            agent_id,
            at_message_id,
            created,
        )
        .await;
    }
    let project = require_project_view(state, project_id)?;
    let baseline = message_workspace_commit(state, project, &source.current_message_id)?;
    ensure_session_worktree(
        workspace,
        lease.clone(),
        project,
        &source.id,
        &baseline,
        created,
    )
    .await
}

async fn prepare_existing_session_worktree(
    workspace: &dyn ProjectWorkspace,
    lease: Option<Arc<dyn WorkspaceLease>>,
    state: &WorkingSet,
    session_id: &str,
    created: &mut Vec<PathBuf>,
) -> Result<(), ApiError> {
    let session = state
        .sessions
        .iter()
        .find(|session| session.id == session_id)
        .ok_or_else(|| error(ErrorCode::SessionNotFound, "session not found", false))?;
    let project = require_project_view(state, &session.project_id)?;
    let baseline = workspace
        .git_head(Path::new(&session.workdir))
        .await
        .map_err(project_error)?
        .unwrap_or_else(|| project.base_commit.clone());
    ensure_session_worktree(
        workspace,
        lease.clone(),
        project,
        &session.id,
        &baseline,
        created,
    )
    .await
}

async fn ensure_session_worktree(
    workspace: &dyn ProjectWorkspace,
    lease: Option<Arc<dyn WorkspaceLease>>,
    project: &ProjectView,
    session_id: &str,
    baseline: &str,
    created: &mut Vec<PathBuf>,
) -> Result<(), ApiError> {
    let worktree = session_worktree_path(&project.workdir, session_id)?;
    if workspace
        .ensure_session_worktree(Path::new(&project.workdir), &worktree, baseline, lease)
        .await
        .map_err(project_error)?
        && !created.contains(&worktree)
    {
        created.push(worktree);
    }
    Ok(())
}
