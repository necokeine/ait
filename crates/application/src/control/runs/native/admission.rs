//! Resolve new and existing Codex Sessions without publishing speculative input.
use ait_contracts::{AgentMode, ApiError, Command};
use ait_domain::{ErrorCode, SessionSource};

use crate::control::{
    LocalControlService,
    catalog::{AgentRecord, require_agent, validate_config},
    conversation::{ConversationContext, SessionRecord, create_session, derive_reuses_source},
    errors::error,
    persistence::transaction::RecordTransaction,
    settings::resolve_project_agent_id,
};

pub(super) struct Plan {
    pub loaded: RecordTransaction<ConversationContext>,
    pub session: SessionRecord,
    pub agent: AgentRecord,
    pub new_session: bool,
    pub text: String,
}

/// Rejects unverified writers, model mismatches, and Threads bound to another Session.
pub(super) fn validate_writer(
    state: &ConversationContext,
    run: &super::RunRecord,
    snapshot: &ait_ports::CodexThreadSnapshot,
    prepared: &ait_ports::CodexPreparedThread,
) -> Result<(), ApiError> {
    if !snapshot.writer_confirmed || prepared.model != run.config.model {
        return Err(error(
            ErrorCode::CodexThreadNotSynced,
            "native writer has not confirmed an idle history",
            false,
        ));
    }
    if state.sessions.iter().any(|session| {
        Some(&session.id) != run.session_id.as_ref()
            && matches!(&session.source, SessionSource::CodexThread(source)
                if source.provider_id == run.provider.id && source.thread_id == snapshot.id)
    }) {
        return Err(error(
            ErrorCode::CodexThreadBindingConflict,
            "Thread is already bound to another Session",
            false,
        ));
    }
    Ok(())
}

fn unsupported() -> ApiError {
    error(
        ErrorCode::CodexForkBoundaryUnsupported,
        "Codex history branching requires native Thread fork support; create a new task from the Project root",
        false,
    )
}

impl LocalControlService {
    pub(super) async fn native_plan(
        &self,
        command: &Command,
        derive_source_locked: bool,
    ) -> Result<Option<Plan>, ApiError> {
        let project_id = match command {
            Command::SendMessage { session_id, .. } => {
                let sessions = self.records().read_session_records(session_id).await?;
                sessions
                    .original
                    .sessions
                    .iter()
                    .find(|session| session.id == *session_id)
                    .ok_or_else(|| error(ErrorCode::SessionNotFound, "Session not found", false))?
                    .project_id
                    .clone()
            }
            Command::ForkSession { project_id, .. } | Command::DeriveSession { project_id, .. } => {
                project_id.clone()
            }
            _ => return Ok(None),
        };
        let loaded = self.native_records(&project_id).await?;
        let mut state = loaded.original.clone();
        let (session_id, text, new_session) = match command {
            Command::SendMessage { session_id, text } => (session_id.clone(), text.clone(), false),
            Command::ForkSession {
                id,
                project_id,
                agent_id,
                at_message_id,
                text,
            }
            | Command::DeriveSession {
                id,
                project_id,
                agent_id,
                at_message_id,
                text,
                ..
            } => {
                let selected = resolve_project_agent_id(&state, project_id, agent_id)?;
                let agent = require_agent(&state, &selected)?;
                if validate_config(&state, &agent.config)?.kind != AgentMode::Codex {
                    return Ok(None);
                }
                if let Command::DeriveSession {
                    source_session_id, ..
                } = command
                {
                    let source = state
                        .sessions
                        .iter()
                        .find(|session| session.id == *source_session_id)
                        .ok_or_else(|| {
                            error(
                                ErrorCode::SessionNotFound,
                                "source Session not found",
                                false,
                            )
                        })?;
                    if derive_source_locked
                        && derive_reuses_source(
                            &state,
                            id,
                            project_id,
                            source,
                            &selected,
                            at_message_id,
                        )
                    {
                        return Self::existing_native_plan(loaded, source_session_id, text)
                            .map(Some);
                    }
                }
                create_native_session(&mut state, id, project_id, &selected, at_message_id)?;
                (id.clone(), text.clone(), true)
            }
            _ => return Ok(None),
        };
        let session = state
            .sessions
            .iter()
            .find(|session| session.id == session_id)
            .ok_or_else(|| error(ErrorCode::SessionNotFound, "Session not found", false))?
            .clone();
        let agent = require_agent(&state, session.agent_id())?.clone();
        if validate_config(&state, &agent.config)?.kind != AgentMode::Codex {
            return Ok(None);
        }
        validate_unbound(&state, &session)?;
        Ok(Some(Plan {
            loaded,
            session,
            agent,
            new_session,
            text,
        }))
    }

    fn existing_native_plan(
        loaded: RecordTransaction<ConversationContext>,
        session_id: &str,
        text: &str,
    ) -> Result<Plan, ApiError> {
        let session = loaded
            .original
            .sessions
            .iter()
            .find(|session| session.id == session_id)
            .ok_or_else(|| error(ErrorCode::SessionNotFound, "Session not found", false))?
            .clone();
        let agent = require_agent(&loaded.original, session.agent_id())?.clone();
        validate_unbound(&loaded.original, &session)?;
        Ok(Plan {
            loaded,
            session,
            agent,
            new_session: false,
            text: text.into(),
        })
    }
}

fn validate_unbound(state: &ConversationContext, session: &SessionRecord) -> Result<(), ApiError> {
    if matches!(session.source, SessionSource::Managed) {
        let project = state
            .projects
            .iter()
            .find(|project| project.id == session.project_id)
            .ok_or_else(|| error(ErrorCode::InvalidProject, "Project not found", false))?;
        if session.current_message_id() != project.root_message_id {
            return Err(unsupported());
        }
    }
    Ok(())
}

fn create_native_session(
    state: &mut ConversationContext,
    id: &str,
    project_id: &str,
    agent_id: &str,
    at_message_id: &str,
) -> Result<(), ApiError> {
    let project = state
        .projects
        .iter()
        .find(|project| project.id == project_id)
        .ok_or_else(|| error(ErrorCode::InvalidProject, "Project not found", false))?;
    if project.root_message_id != at_message_id {
        return Err(unsupported());
    }
    create_session(
        state,
        id.into(),
        project_id.into(),
        agent_id,
        Some(at_message_id.into()),
    )
    .map(|_| ())
}

pub(super) fn stage_session(
    state: &mut ConversationContext,
    session: &SessionRecord,
    agent: &AgentRecord,
    new_session: bool,
) -> Result<usize, ApiError> {
    if new_session {
        if state
            .sessions
            .iter()
            .any(|existing| existing.id == session.id)
        {
            return Err(super::conflict());
        }
        state.sessions.push(session.clone());
        if !state.agents.iter().any(|existing| existing.id == agent.id) {
            state.agents.push(agent.clone());
        }
    }
    state
        .sessions
        .iter()
        .position(|existing| existing.id == session.id)
        .ok_or_else(|| {
            error(
                ErrorCode::SessionNotFound,
                "native Session disappeared",
                false,
            )
        })
}
