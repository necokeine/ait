//! Project registration and default Agent selection.
use crate::control::catalog::require_named_agent;
use crate::control::errors::error;
use crate::control::events::{now, pending};
use crate::control::project::git::{ensure_git_head, prepare_git_root};
use crate::control::state::WorkingSet;
use ait_contracts::{ApiError, CommandResult, MessageView, ProjectView};
use ait_domain::ErrorCode;
use ait_ports::PendingEvent;
use std::path::Path;
use uuid::Uuid;

pub(in crate::control) mod archive;
pub(in crate::control) mod git;
pub(in crate::control) mod worktrees;

pub(in crate::control) fn set_project_default_agent(
    state: &mut WorkingSet,
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

pub(in crate::control) fn validate_project_registration(
    state: &WorkingSet,
    id: &str,
    name: &str,
    repo_url: &mut Option<String>,
) -> Result<(), ApiError> {
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
    if let Some(url) = repo_url {
        *url = url.trim().to_owned();
        if url.is_empty() {
            return Err(error(
                ErrorCode::InvalidProject,
                "repository URL cannot be empty",
                false,
            ));
        }
    }
    Ok(())
}

pub(in crate::control) fn register_project(
    state: &mut WorkingSet,
    id: String,
    name: String,
    workdir: &str,
    mut repo_url: Option<String>,
) -> Result<(CommandResult, Vec<PendingEvent>), ApiError> {
    validate_project_registration(state, &id, &name, &mut repo_url)?;
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

pub(in crate::control) fn require_project_view<'a>(
    state: &'a WorkingSet,
    project_id: &str,
) -> Result<&'a ProjectView, ApiError> {
    state
        .projects
        .iter()
        .find(|project| project.id == project_id)
        .ok_or_else(|| error(ErrorCode::InvalidProject, "project not found", false))
}
