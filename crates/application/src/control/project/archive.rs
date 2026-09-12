//! Project archive import, export and validation.
use crate::control::catalog::validate_provider;
use crate::control::errors::error;
use crate::control::events::pending;
use crate::control::project::git::{ensure_git_head, is_git_commit, prepare_git_root};
use crate::control::project::worktrees::session_worktree_path;
use crate::control::state::WorkingSet;
#[cfg(all(feature = "dev-mock-provider", debug_assertions))]
use ait_contracts::AgentMode;
use ait_contracts::{
    AgentProviderView, ApiError, CommandResult, PROJECT_EXPORT_VERSION, ProjectExport,
};
use ait_domain::ErrorCode;
use ait_ports::PendingEvent;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use uuid::Uuid;

pub(in crate::control) fn export_project(
    state: &WorkingSet,
    source_revision: u64,
    project_id: &str,
) -> Result<ProjectExport, ApiError> {
    let project = state
        .projects
        .iter()
        .find(|project| project.id == project_id)
        .cloned()
        .ok_or_else(|| error(ErrorCode::InvalidProject, "project not found", false))?;
    let messages = state
        .messages
        .iter()
        .filter(|message| message.project_id == project_id)
        .cloned()
        .map(|mut message| {
            if message
                .data
                .as_ref()
                .is_some_and(|d| d.get("native_message").is_some())
            {
                message.data = None;
                message.text.get_or_insert_with(|| {
                    "[Host tool payload omitted from portable archive]".into()
                });
            }
            message
        })
        .collect::<Vec<_>>();
    let sessions = state
        .sessions
        .iter()
        .filter(|session| session.project_id == project_id)
        .cloned()
        .map(|mut session| {
            // An active Run is process-local state and cannot safely be resumed
            // from a portable archive.
            session.active_run_id = None;
            // Session worktrees are host-local and are recreated below the
            // destination Project's `.ait` directory on import.
            session.workdir.clear();
            session
        })
        .collect::<Vec<_>>();
    let mut referenced_agents = sessions
        .iter()
        .map(|session| session.agent_id.as_str())
        .collect::<HashSet<_>>();
    if let Some(default_agent_id) = project.default_agent_id.as_deref() {
        referenced_agents.insert(default_agent_id);
    }
    let agents: Vec<_> = state
        .agents
        .iter()
        .filter(|agent| referenced_agents.contains(agent.id.as_str()))
        .cloned()
        .collect();
    let providers = state
        .providers
        .iter()
        .filter(|p| agents.iter().any(|a| a.config.provider_id == p.provider.id))
        .map(|p| p.provider.clone())
        .collect();
    let archive = ProjectExport {
        format_version: PROJECT_EXPORT_VERSION,
        source_revision,
        providers,
        project,
        agents,
        sessions,
        messages,
    };
    validate_project_export(&archive)?;
    Ok(archive)
}

pub(in crate::control) fn import_project(
    state: &mut WorkingSet,
    archive: ProjectExport,
    workdir: &str,
) -> Result<(CommandResult, Vec<PendingEvent>), ApiError> {
    validate_project_export(&archive)?;
    validate_import_conflicts(state, &archive)?;
    let canonical = prepare_git_root(Path::new(workdir))?;
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
    let mut project = archive.project;
    project.workdir = canonical_text;
    project.base_commit = ensure_git_head(&canonical)?;
    let mut sessions = archive.sessions;
    for session in &mut sessions {
        session.workdir = session_worktree_path(&project.workdir, &session.id)?
            .to_string_lossy()
            .into_owned();
    }
    for agent in archive.agents {
        if !state.agents.iter().any(|existing| existing.id == agent.id) {
            state.agents.push(agent);
        }
    }
    for provider in archive.providers {
        if !state.providers.iter().any(|p| p.provider.id == provider.id) {
            state.providers.push(AgentProviderView {
                provider,
                has_secret: false,
            });
        }
    }
    state.messages.extend(archive.messages);
    state.sessions.extend(sessions);
    state.projects.push(project.clone());
    Ok((
        CommandResult::Project(project.clone()),
        vec![pending(
            "project.imported",
            Some(project.id.clone()),
            &project,
        )],
    ))
}

pub(in crate::control) fn validate_import_conflicts(
    state: &WorkingSet,
    archive: &ProjectExport,
) -> Result<(), ApiError> {
    if state
        .projects
        .iter()
        .any(|project| project.id == archive.project.id)
    {
        return Err(error(
            ErrorCode::InvalidProject,
            "project id already exists",
            false,
        ));
    }
    if archive.messages.iter().any(|imported| {
        state
            .messages
            .iter()
            .any(|existing| existing.id == imported.id)
    }) || archive.sessions.iter().any(|imported| {
        state
            .sessions
            .iter()
            .any(|existing| existing.id == imported.id)
    }) {
        return Err(error(
            ErrorCode::InvalidProject,
            "archive identity conflicts with existing workspace state",
            false,
        ));
    }
    for imported in &archive.agents {
        if let Some(existing) = state
            .agents
            .iter()
            .find(|existing| existing.id == imported.id)
            && existing != imported
        {
            return Err(error(
                ErrorCode::InvalidAgentConfiguration,
                "archive agent conflicts with an existing revision",
                false,
            ));
        }
    }

    for provider in &archive.providers {
        if let Some(existing) = state
            .providers
            .iter()
            .find(|p| p.provider.id == provider.id)
            && existing.provider != *provider
        {
            return Err(invalid_archive(
                "archive provider conflicts with an existing connection",
            ));
        }
    }
    Ok(())
}

pub(in crate::control) fn validate_project_export(archive: &ProjectExport) -> Result<(), ApiError> {
    if archive.format_version != PROJECT_EXPORT_VERSION
        || archive.source_revision == 0
        || archive.project.id.trim().is_empty()
        || archive.project.revision == 0
        || !is_git_commit(&archive.project.base_commit)
        || archive
            .project
            .repo_url
            .as_ref()
            .is_some_and(|url| url.trim().is_empty())
        || archive.messages.is_empty()
    {
        return Err(invalid_archive(
            "archive header or project revision is invalid",
        ));
    }

    let mut message_by_id = HashMap::with_capacity(archive.messages.len());
    for message in &archive.messages {
        if message.project_id != archive.project.id
            || Uuid::parse_str(&message.id).is_err()
            || (message.role == "user"
                && message.kind == "standard"
                && message
                    .git_commit
                    .as_deref()
                    .is_none_or(|commit| !is_git_commit(commit)))
            || (message.git_commit.is_some()
                && (message.role != "user" || message.kind != "standard"))
            || message_by_id.insert(message.id.as_str(), message).is_some()
        {
            return Err(invalid_archive(
                "archive message identity or project ownership is invalid",
            ));
        }
    }
    let Some(root) = message_by_id.get(archive.project.root_message_id.as_str()) else {
        return Err(invalid_archive("archive root message is missing"));
    };
    if root.parent_message_id.is_some() || root.role != "system" {
        return Err(invalid_archive("archive root message is invalid"));
    }
    for message in &archive.messages {
        let mut cursor = message;
        let mut seen = HashSet::new();
        while cursor.id != archive.project.root_message_id {
            if !seen.insert(cursor.id.as_str()) {
                return Err(invalid_archive("archive message graph contains a cycle"));
            }
            let Some(parent_id) = cursor.parent_message_id.as_deref() else {
                return Err(invalid_archive(
                    "archive message graph contains an unexpected root",
                ));
            };
            cursor = message_by_id
                .get(parent_id)
                .copied()
                .ok_or_else(|| invalid_archive("archive message parent is missing"))?;
        }
    }

    validate_archive_catalog(archive)?;
    let mut agent_ids = HashSet::with_capacity(archive.agents.len());
    if archive.agents.iter().any(|agent| {
        agent.id.trim().is_empty() || agent.revision == 0 || !agent_ids.insert(agent.id.as_str())
    }) {
        return Err(invalid_archive("archive agent revision is invalid"));
    }
    if archive
        .project
        .default_agent_id
        .as_deref()
        .is_some_and(|agent_id| !agent_ids.contains(agent_id))
    {
        return Err(invalid_archive(
            "archive Project default Agent binding is invalid",
        ));
    }
    let mut session_ids = HashSet::with_capacity(archive.sessions.len());
    for session in &archive.sessions {
        if session.project_id != archive.project.id
            || session.version == 0
            || session.active_run_id.is_some()
            || !session_ids.insert(session.id.as_str())
            || !message_by_id.contains_key(session.current_message_id.as_str())
            || !agent_ids.contains(session.agent_id.as_str())
        {
            return Err(invalid_archive(
                "archive session pointer, agent binding, or revision is invalid",
            ));
        }
    }
    Ok(())
}

fn invalid_archive(message: impl Into<String>) -> ApiError {
    error(ErrorCode::InvalidProject, message, false)
}

fn validate_archive_catalog(archive: &ProjectExport) -> Result<(), ApiError> {
    let mut provider_ids = HashSet::new();
    for provider in &archive.providers {
        validate_provider(provider)?;
        #[cfg(all(feature = "dev-mock-provider", debug_assertions))]
        if provider.kind == AgentMode::Mock {
            return Err(invalid_archive(
                "development Mock providers cannot be imported or exported",
            ));
        }
        if !provider_ids.insert(&provider.id) {
            return Err(invalid_archive("duplicate provider"));
        }
    }
    for agent in &archive.agents {
        // Archives preserve configuration even when a provider has delisted its model.
        // Availability is checked when starting new work, not when copying history.
        if !provider_ids.contains(&agent.config.provider_id)
            || agent.config.model.trim().is_empty()
            || agent
                .config
                .reasoning_effort
                .as_ref()
                .is_some_and(|effort| effort.trim().is_empty())
        {
            return Err(invalid_archive(
                "invalid archived Agent configuration or provider reference",
            ));
        }
        if let Some(owner) = &agent.owner_session_id {
            if !agent.name.is_empty()
                || !archive
                    .sessions
                    .iter()
                    .any(|s| &s.id == owner && s.agent_id == agent.id)
            {
                return Err(invalid_archive("invalid anonymous Agent owner"));
            }
        } else if agent.name.trim().is_empty() {
            return Err(invalid_archive("named Agent requires a name"));
        }
    }
    if archive.project.default_agent_id.as_ref().is_some_and(|id| {
        archive
            .agents
            .iter()
            .any(|a| &a.id == id && a.owner_session_id.is_some())
    }) {
        return Err(invalid_archive(
            "Project default Agent must be a named preset",
        ));
    }
    for session in &archive.sessions {
        if archive.agents.iter().any(|a| {
            a.id == session.agent_id
                && a.owner_session_id
                    .as_ref()
                    .is_some_and(|owner| owner != &session.id)
        }) {
            return Err(invalid_archive(
                "anonymous Agent cannot be shared between Sessions",
            ));
        }
    }
    Ok(())
}
