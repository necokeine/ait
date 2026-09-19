//! Session creation, derivation, binding and pointer ownership.
use crate::control::runs::RunRecord;

use crate::control::catalog::{agent_for_session, require_agent};
use crate::control::conversation::messages::send_message;
use crate::control::errors::error;
use crate::control::events::pending;
use crate::control::execution::CommandOutcome;
use crate::control::permissions::PermissionPolicyLimits;
use crate::control::persistence::{
    HasAgents, HasMessages, HasProjects, HasProviderCredentials, HasProviders, HasRunCredentials,
    HasRuns, HasSessions, HasSettings,
};
use crate::control::project::git::GitBaseline;
use crate::control::project::worktrees::{session_worktree_path, validate_session_path_component};
use crate::control::settings::resolve_project_agent_id;
use ait_contracts::{ApiError, CommandResult};
use ait_domain::{ErrorCode, SessionStatus};
use ait_ports::PendingEvent;

pub(in crate::control) mod messages;
pub(in crate::control) mod title;

pub(in crate::control) struct ForkSessionInput {
    pub(in crate::control) id: String,
    pub(in crate::control) project_id: String,
    pub(in crate::control) agent_id: String,
    pub(in crate::control) at_message_id: String,
    pub(in crate::control) text: String,
}

pub(in crate::control) fn fork_session(
    state: &mut (
             impl HasAgents
             + HasMessages
             + HasProjects
             + HasProviderCredentials
             + HasProviders
             + HasRunCredentials
             + HasRuns
             + HasSessions
             + HasSettings
         ),
    input: ForkSessionInput,
    git_baseline: &GitBaseline,
    permission_limits: PermissionPolicyLimits,
) -> Result<(CommandOutcome, Vec<PendingEvent>), ApiError> {
    let (_, mut events) = create_session(
        state,
        input.id.clone(),
        input.project_id,
        &input.agent_id,
        Some(input.at_message_id),
    )?;
    let (result, mut run_events) = send_message(
        state,
        input.id,
        input.text,
        Some(git_baseline),
        permission_limits,
    )?;
    events.append(&mut run_events);
    Ok((result, events))
}

pub(in crate::control) fn derive_session(
    state: &mut (
             impl HasAgents
             + HasMessages
             + HasProjects
             + HasProviderCredentials
             + HasProviders
             + HasRunCredentials
             + HasRuns
             + HasSessions
             + HasSettings
         ),
    input: ForkSessionInput,
    source_session_id: &str,
    source_locked: bool,
    git_baseline: &GitBaseline,
    permission_limits: PermissionPolicyLimits,
) -> Result<(CommandOutcome, Vec<PendingEvent>), ApiError> {
    if input.text.trim().is_empty() {
        return Err(error(
            ErrorCode::InvalidMessageRole,
            "message text is required",
            false,
        ));
    }
    if input.id.trim().is_empty()
        || state
            .sessions()
            .iter()
            .any(|session| session.id == input.id)
    {
        return Err(error(
            ErrorCode::InvalidSession,
            "session id is empty or already exists",
            false,
        ));
    }
    if !state
        .projects()
        .iter()
        .any(|project| project.id == input.project_id)
    {
        return Err(error(ErrorCode::InvalidProject, "project not found", false));
    }
    let selected_agent_id = resolve_project_agent_id(state, &input.project_id, &input.agent_id)?;
    require_agent(state, &selected_agent_id)?;
    let source_message = state
        .messages()
        .iter()
        .find(|message| message.id == input.at_message_id)
        .ok_or_else(|| {
            error(
                ErrorCode::MessageNotFound,
                "branch message not found",
                false,
            )
        })?;
    if source_message.project_id != input.project_id {
        return Err(error(
            ErrorCode::SessionMessageProjectMismatch,
            "branch message belongs to another project",
            false,
        ));
    }
    let source_session = state
        .sessions()
        .iter()
        .find(|session| session.id == source_session_id)
        .ok_or_else(|| error(ErrorCode::SessionNotFound, "session not found", false))?
        .clone();
    if source_session.project_id != input.project_id {
        return Err(error(
            ErrorCode::SessionMessageProjectMismatch,
            "source Session belongs to another project",
            false,
        ));
    }

    let can_reuse = source_locked
        && derive_reuses_source(
            state,
            &input.id,
            &input.project_id,
            &source_session,
            &selected_agent_id,
            &input.at_message_id,
        );
    if can_reuse {
        return send_message(
            state,
            source_session_id.to_owned(),
            input.text,
            Some(git_baseline),
            permission_limits,
        );
    }
    fork_session(
        state,
        ForkSessionInput {
            agent_id: selected_agent_id,
            ..input
        },
        git_baseline,
        permission_limits,
    )
}

pub(in crate::control) fn create_session(
    state: &mut (impl HasAgents + HasMessages + HasProjects + HasSessions + HasSettings),
    id: String,
    project_id: String,
    agent_id: &str,
    at_message_id: Option<String>,
) -> Result<(CommandResult, Vec<PendingEvent>), ApiError> {
    validate_session_path_component(&id)?;
    if state.sessions().iter().any(|session| session.id == id) {
        return Err(error(
            ErrorCode::InvalidSession,
            "session id is empty or already exists",
            false,
        ));
    }
    let agent_id = resolve_project_agent_id(state, &project_id, agent_id)?;
    let project = state
        .projects()
        .iter()
        .find(|project| project.id == project_id)
        .ok_or_else(|| error(ErrorCode::InvalidProject, "project not found", false))?;
    let project_workdir = project.workdir.clone();
    require_agent(state, &agent_id)?;
    let head = at_message_id.unwrap_or_else(|| project.root_message_id.clone());
    let target = state
        .messages()
        .iter()
        .find(|message| message.id == head)
        .ok_or_else(|| {
            error(
                ErrorCode::MessageNotFound,
                "branch message not found",
                false,
            )
        })?;
    if target.project_id != project_id {
        return Err(error(
            ErrorCode::SessionMessageProjectMismatch,
            "branch message belongs to another project",
            false,
        ));
    }
    let session_workdir = session_worktree_path(&project_workdir, &id)?;
    let agent_id = agent_for_session(state, &agent_id, &id)?;
    let session = SessionRecord {
        id: id.clone(),
        project_id,
        workdir: session_workdir.to_string_lossy().into_owned(),
        source: ait_domain::SessionSource::Managed,
        name: String::new(),
        title: None,
        description: String::new(),
        title_generation_started: false,
        status: SessionStatus::Active,
        reference: ait_domain::SessionReference::new(
            ait_domain::MessageId::parse(&head).map_err(|_| {
                error(
                    ErrorCode::InvalidMessageId,
                    "invalid Message identity",
                    false,
                )
            })?,
            ait_domain::AgentId::new(agent_id),
        ),
    };
    state.sessions_mut().push(session.clone());
    Ok((
        CommandResult::Session(session.view()),
        vec![pending("session.created", Some(id), &session)],
    ))
}

pub(in crate::control) fn derive_reuses_source(
    state: &impl HasMessages,
    _requested_id: &str,
    project_id: &str,
    source: &SessionRecord,
    agent_id: &str,
    at_message_id: &str,
) -> bool {
    source.project_id == project_id
        && source.status == SessionStatus::Active
        && source.active_run_id().is_none()
        && source.current_message_id() == at_message_id
        && source.agent_id() == agent_id
        && !state
            .messages()
            .iter()
            .any(|message| message.parent_message_id.as_deref() == Some(at_message_id))
}

pub(in crate::control) fn rename_session(
    state: &mut impl HasSessions,
    session_id: &str,
    name: &str,
) -> Result<(CommandResult, Vec<PendingEvent>), ApiError> {
    let name = name.split_whitespace().collect::<Vec<_>>().join(" ");
    if name.chars().count() > 100 {
        return Err(error(
            ErrorCode::InvalidSession,
            "Session name must be at most 100 characters",
            false,
        ));
    }
    let session = state
        .sessions_mut()
        .iter_mut()
        .find(|session| session.id == session_id)
        .ok_or_else(|| error(ErrorCode::SessionNotFound, "session not found", false))?;
    session.name = name;
    let session = session.clone();
    Ok((
        CommandResult::Session(session.view()),
        vec![pending(
            "session.renamed",
            Some(session_id.to_owned()),
            &session,
        )],
    ))
}

pub(in crate::control) fn set_session_archived(
    state: &mut impl HasSessions,
    session_id: &str,
    archived: bool,
) -> Result<(CommandResult, Vec<PendingEvent>), ApiError> {
    let session = state
        .sessions_mut()
        .iter_mut()
        .find(|session| session.id == session_id)
        .ok_or_else(|| error(ErrorCode::SessionNotFound, "session not found", false))?;
    if archived && session.active_run_id().is_some() {
        return Err(error(
            ErrorCode::SessionBusy,
            "session already has an active run",
            false,
        ));
    }
    let status = if archived {
        SessionStatus::Archived
    } else {
        SessionStatus::Active
    };
    if session.status == status {
        return Ok((CommandResult::Session(session.view()), Vec::new()));
    }
    session.status = status;
    let session = session.clone();
    let event = if archived {
        "session.archived"
    } else {
        "session.restored"
    };
    Ok((
        CommandResult::Session(session.view()),
        vec![pending(event, Some(session_id.to_owned()), &session)],
    ))
}

pub(in crate::control) fn set_session_agent(
    state: &mut (impl HasAgents + HasSessions),
    session_id: &str,
    agent_id: &str,
) -> Result<(CommandResult, Vec<PendingEvent>), ApiError> {
    let requested = require_agent(state, agent_id)?;
    if let Some(session) = state
        .sessions()
        .iter()
        .find(|session| session.id == session_id)
        && let ait_domain::SessionSource::CodexThread(source) = &session.source
        && requested.config.provider_id != source.provider_id
    {
        return Err(error(
            ErrorCode::CodexThreadBindingConflict,
            "A native Codex task cannot switch providers",
            false,
        ));
    }
    let agent_id = agent_for_session(state, agent_id, session_id)?;
    let session = state
        .sessions_mut()
        .iter_mut()
        .find(|session| session.id == session_id)
        .ok_or_else(|| error(ErrorCode::SessionNotFound, "session not found", false))?;
    if session.active_run_id().is_some() {
        return Err(error(
            ErrorCode::SessionBusy,
            "session already has an active run",
            false,
        ));
    }
    if session.agent_id() == agent_id {
        return Ok((CommandResult::Session(session.view()), Vec::new()));
    }
    session
        .reference
        .bind(ait_domain::AgentId::new(agent_id))
        .map_err(|e| error(e.code, e.message, e.retryable))?;
    let session = session.clone();
    Ok((
        CommandResult::Session(session.view()),
        vec![pending(
            "session.agent_updated",
            Some(session_id.to_owned()),
            &session,
        )],
    ))
}

pub(in crate::control) fn release_session(state: &mut impl HasSessions, run: &RunRecord) {
    if let Some(session_id) = &run.session_id
        && let Some(session) = state.sessions_mut().iter_mut().find(|session| {
            &session.id == session_id && session.active_run_id() == Some(run.id.as_str())
        })
    {
        session.reference.release(&ait_domain::RunId::new(&run.id));
    }
}
mod context;
mod message_record;
mod record;
pub(in crate::control) use context::{
    CodexImportContext, ConversationContext, MessagesContext, NewSessionContext,
    SessionBindingContext, SessionConfigContext, SessionTitleContext, SessionsContext,
};
pub(in crate::control) use message_record::domain_path;
pub(in crate::control) use record::{MessageRecord, SessionRecord};
