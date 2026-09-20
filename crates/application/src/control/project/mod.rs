//! Project registration and default Agent selection.
use crate::control::catalog::require_named_agent;
use crate::control::conversation::MessageRecord;
use crate::control::errors::error;
use crate::control::events::{now, pending};
use crate::control::persistence::{HasAgents, HasMessages, HasProjects};
use crate::control::project::git::PreparedProject;
use ait_contracts::{ApiError, CommandResult};
use ait_domain::ErrorCode;
use ait_ports::PendingEvent;
use uuid::Uuid;

mod execution;
mod fenced_store;
pub(in crate::control) mod git;
mod lifecycle;
pub(in crate::control) mod worktrees;

pub(in crate::control) fn update_project(
    state: &mut (impl HasAgents + HasProjects),
    project_id: &str,
    name: &str,
    agent_id: Option<&str>,
) -> Result<(CommandResult, Vec<PendingEvent>), ApiError> {
    let name = name.trim();
    ait_domain::project::validate_registration(project_id, name, &mut None)
        .map_err(|e| error(e.code, e.message, e.retryable))?;
    if let Some(agent_id) = agent_id.filter(|id| !id.trim().is_empty()) {
        require_named_agent(state, agent_id)?;
    }
    let project = state
        .projects_mut()
        .iter_mut()
        .find(|project| project.id == project_id)
        .ok_or_else(|| error(ErrorCode::InvalidProject, "project not found", false))?;
    name.clone_into(&mut project.name);
    if let Some(agent_id) = agent_id {
        if agent_id.trim().is_empty() {
            project.defaults.clear();
        } else {
            project.defaults.select(ait_domain::AgentId::new(agent_id));
        }
    } else {
        project.defaults.mark_updated();
    }
    Ok((
        CommandResult::Project(project.view()),
        vec![pending(
            "project.updated",
            Some(project_id.to_owned()),
            project,
        )],
    ))
}

pub(in crate::control) fn set_project_default_agent(
    state: &mut (impl HasAgents + HasProjects),
    project_id: &str,
    agent_id: &str,
) -> Result<(CommandResult, Vec<PendingEvent>), ApiError> {
    require_named_agent(state, agent_id)?;
    let project = state
        .projects_mut()
        .iter_mut()
        .find(|project| project.id == project_id)
        .ok_or_else(|| error(ErrorCode::InvalidProject, "project not found", false))?;
    project.defaults.select(ait_domain::AgentId::new(agent_id));
    let project = project.clone();
    Ok((
        CommandResult::Project(project.view()),
        vec![pending(
            "project.default_agent_updated",
            Some(project_id.to_owned()),
            &project,
        )],
    ))
}

pub(in crate::control) fn validate_project_registration(
    state: &impl HasProjects,
    id: &str,
    name: &str,
    repo_url: &mut Option<String>,
) -> Result<(), ApiError> {
    ait_domain::project::validate_registration(id, name, repo_url)
        .map_err(|e| error(e.code, e.message, e.retryable))?;
    if state.projects().iter().any(|project| project.id == id) {
        return Err(error(
            ErrorCode::InvalidProject,
            "project id already exists",
            false,
        ));
    }
    Ok(())
}

pub(in crate::control) fn register_project(
    state: &mut (impl HasMessages + HasProjects),
    id: String,
    name: String,
    prepared: &PreparedProject,
    mut repo_url: Option<String>,
) -> Result<(CommandResult, Vec<PendingEvent>), ApiError> {
    validate_project_registration(state, &id, &name, &mut repo_url)?;
    let base_commit = prepared.base_commit.clone();
    let canonical_text = prepared.workdir.clone();
    validate_project_workdir(state, &canonical_text)?;
    let root_id = Uuid::new_v4().to_string();
    let project = ProjectRecord {
        owner: None,
        execution_blocked: None,
        id: id.clone(),
        name,
        workdir: canonical_text,
        root_message_id: root_id.clone(),
        repo_url,
        base_commit,
        defaults: ait_domain::ProjectDefaults::default(),
    };
    state.messages_mut().push(MessageRecord {
        id: root_id,
        project_id: id.clone(),
        parent_message_id: None,
        role: ait_domain::MessageRole::System,
        kind: ait_domain::MessageKind::Standard,
        text: Some("AIT project instructions".into()),
        created_at: now(),
        git_commit: None,
        data: None,
    });
    state.projects_mut().push(project.clone());
    Ok((
        CommandResult::Project(project.view()),
        vec![pending("project.registered", Some(id), &project)],
    ))
}

pub(in crate::control) fn validate_project_workdir(
    state: &impl HasProjects,
    canonical_workdir: &str,
) -> Result<(), ApiError> {
    if state
        .projects()
        .iter()
        .any(|project| project.workdir == canonical_workdir)
    {
        return Err(error(
            ErrorCode::ProjectPathAlreadyRegistered,
            "project path is already registered",
            false,
        ));
    }
    Ok(())
}

pub(in crate::control) fn require_project_view<'a>(
    state: &'a impl HasProjects,
    project_id: &str,
) -> Result<&'a ProjectRecord, ApiError> {
    state
        .projects()
        .iter()
        .find(|project| project.id == project_id)
        .ok_or_else(|| error(ErrorCode::InvalidProject, "project not found", false))
}
mod context;
mod record;
pub(in crate::control) use context::{
    ProjectAgentContext, ProjectRegistrationContext, ProjectsContext,
};
pub(in crate::control) use record::ProjectRecord;
