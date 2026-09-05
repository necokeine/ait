use std::{collections::HashSet, sync::Arc};

use ait_domain::{
    AgentId, Message, MessageId, MessageKind, MessageRole, MessageValidationError, ProjectId,
    ProjectedMessage, Session, SessionId, TimestampMs,
};
use ait_ports::{MessageStore, MessageStoreError, SessionAdvance, SessionStore, SessionStoreError};
use thiserror::Error;

/// Ordered root-to-head projection of a Session's current branch.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionView {
    /// Session ref used for the projection.
    pub session: Session,
    /// Ordered path including the root and current head.
    pub messages: Vec<ProjectedMessage>,
}

/// Stable Message/Session service failures.
#[derive(Debug, Error)]
pub enum MessageServiceError {
    /// A Message failed local protocol validation.
    #[error(transparent)]
    Validation(#[from] MessageValidationError),
    /// A non-root append did not name a parent.
    #[error("a non-root message requires a parent")]
    ParentRequired,
    /// A Message or Session target belongs to another Project.
    #[error(
        "message belongs to project {actual}, expected {expected}",
        actual = .actual.as_str(),
        expected = .expected.as_str()
    )]
    ProjectMismatch {
        /// Project required by the operation.
        expected: ProjectId,
        /// Project found on the target.
        actual: ProjectId,
    },
    /// Imported or corrupted storage contains a parent cycle.
    #[error("message parent cycle detected at {0}")]
    CycleDetected(MessageId),
    /// A path terminated at a non-System root.
    #[error("message path terminated at an invalid root")]
    InvalidRoot,
    /// A Session-bound Message was not a direct child of the expected head.
    #[error("session advancement requires a direct child of the expected head")]
    NotDirectChild,
    /// Editing may only replace an immutable standard Message with an equivalent sibling.
    #[error("edit replacement is not a same-role standard sibling")]
    InvalidEditFork,
    /// Regeneration may only replace a standard assistant Message with an assistant sibling.
    #[error("regeneration replacement is not an assistant sibling")]
    InvalidRegenerationFork,
    /// The append committed but the Session compare-and-swap lost a race.
    #[error(
        "session pointer conflict; message {preserved_message_id} was preserved",
        preserved_message_id = .preserved_message_id
    )]
    PointerConflict {
        /// Current Session state observed by persistence.
        observed: Box<Session>,
        /// Newly appended Message retained as a sibling branch.
        preserved_message_id: MessageId,
    },
    /// Message persistence failure.
    #[error(transparent)]
    MessageStore(#[from] MessageStoreError),
    /// Session persistence failure before any Message was appended.
    #[error(transparent)]
    SessionStore(#[from] SessionStoreError),
    /// Session persistence failed after append; the new Message remains addressable.
    #[error(
        "session update failed after message {preserved_message_id} was preserved: {source}",
        preserved_message_id = .preserved_message_id
    )]
    SessionUpdateFailed {
        /// Underlying safe Session-store failure.
        source: SessionStoreError,
        /// Message already committed before the failed Session update.
        preserved_message_id: MessageId,
    },
}

impl MessageServiceError {
    /// Returns the stable ADR error code for transport mapping.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::Validation(MessageValidationError::InvalidMessageId) => "INVALID_MESSAGE_ID",
            Self::Validation(MessageValidationError::InvalidRootMessage) | Self::InvalidRoot => {
                "INVALID_ROOT_MESSAGE"
            }
            Self::Validation(MessageValidationError::ToolUseRequiresAssistant) => {
                "TOOL_USE_REQUIRES_ASSISTANT"
            }
            Self::Validation(MessageValidationError::ToolResultMessageInvalid) => {
                "TOOL_RESULT_MESSAGE_INVALID"
            }
            Self::Validation(MessageValidationError::InvalidRunProvenance) => {
                "INVALID_MESSAGE_RUN_PROVENANCE"
            }
            Self::Validation(MessageValidationError::InvalidSubMessage) => {
                "INVALID_SUBMESSAGE_KIND"
            }
            Self::Validation(MessageValidationError::ToolResultRequiresUser) => {
                "TOOL_RESULT_REQUIRES_USER"
            }
            Self::ParentRequired | Self::NotDirectChild => "MESSAGE_PARENT_INVALID",
            Self::ProjectMismatch { .. }
            | Self::MessageStore(MessageStoreError::MessageProjectMismatch { .. }) => {
                "MESSAGE_PROJECT_MISMATCH"
            }
            Self::CycleDetected(_) => "MESSAGE_CYCLE_DETECTED",
            Self::InvalidEditFork => "MESSAGE_EDIT_FORK_INVALID",
            Self::InvalidRegenerationFork => "MESSAGE_REGENERATION_FORK_INVALID",
            Self::PointerConflict { .. } => "SESSION_POINTER_CONFLICT",
            Self::MessageStore(MessageStoreError::MessageNotFound(_)) => "MESSAGE_NOT_FOUND",
            Self::SessionStore(SessionStoreError::SessionNotFound(_)) => "SESSION_NOT_FOUND",
            Self::MessageStore(MessageStoreError::IdentityConflict(_))
            | Self::SessionStore(SessionStoreError::IdentityConflict(_)) => "IDENTITY_CONFLICT",
            Self::SessionUpdateFailed { .. } => "SESSION_UPDATE_FAILED",
            Self::MessageStore(MessageStoreError::Other(_))
            | Self::SessionStore(SessionStoreError::Other(_)) => "STORE_ERROR",
        }
    }
}

/// Coordinates immutable Message writes, path projection, and Session ref movement.
pub struct MessageService {
    messages: Arc<dyn MessageStore>,
    sessions: Arc<dyn SessionStore>,
}

impl MessageService {
    /// Creates a service over independent Message and Session persistence boundaries.
    ///
    /// The Message store must already contain its root System Message.
    #[must_use]
    pub fn new(messages: Arc<dyn MessageStore>, sessions: Arc<dyn SessionStore>) -> Self {
        Self { messages, sessions }
    }

    /// Appends an immutable child below any Message in the same Project.
    ///
    /// `created_by_session_id` is audit provenance only; it does not constrain
    /// which same-Project parent may be selected.
    ///
    /// # Errors
    ///
    /// Returns a [`MessageServiceError`] for a missing/cross-Project parent,
    /// invalid protocol content, identity conflict, or persistence failure.
    pub fn append(&self, message: Message) -> Result<Message, MessageServiceError> {
        message.validate()?;
        let parent_id = message
            .parent_message_id
            .as_ref()
            .ok_or(MessageServiceError::ParentRequired)?;
        let parent = self.messages.get_message(parent_id)?;
        require_project(&message.project_id, &parent.message.project_id)?;
        self.messages.append_message(message).map_err(Into::into)
    }

    /// Opens a new Session ref at any Message in the same Project.
    ///
    /// This is the supported history/head switch operation: it does not move an
    /// existing Session or copy Message history.
    ///
    /// # Errors
    ///
    /// Returns a [`MessageServiceError`] when the Message is missing, belongs to
    /// another Project, or the Session cannot be created.
    pub fn open_session(
        &self,
        session_id: SessionId,
        project_id: ProjectId,
        at_message_id: MessageId,
        agent_id: AgentId,
    ) -> Result<Session, MessageServiceError> {
        let target = self.messages.get_message(&at_message_id)?;
        require_project(&project_id, &target.message.project_id)?;
        self.sessions
            .create_session(Session::new(
                session_id,
                project_id,
                "",
                at_message_id,
                agent_id,
                TimestampMs(0),
            ))
            .map_err(Into::into)
    }

    /// Projects the ordered root-to-head path for an arbitrary Message.
    ///
    /// Redacted nodes remain in the path as content-free placeholders, so
    /// descendants and graph continuity are never lost.
    ///
    /// # Errors
    ///
    /// Returns a [`MessageServiceError`] for a missing parent, cross-Project
    /// edge, cycle, or invalid root.
    pub fn message_path(
        &self,
        head: &MessageId,
    ) -> Result<Vec<ProjectedMessage>, MessageServiceError> {
        let mut path = Vec::new();
        let mut seen = HashSet::new();
        let mut current = self.messages.get_message(head)?;
        let project_id = current.message.project_id.clone();

        loop {
            if !seen.insert(current.message.id) {
                return Err(MessageServiceError::CycleDetected(current.message.id));
            }
            require_project(&project_id, &current.message.project_id)?;
            let parent = current.message.parent_message_id;
            path.push(ProjectedMessage::from(current));
            let Some(parent_id) = parent else { break };
            current = self.messages.get_message(&parent_id)?;
        }

        path.reverse();
        if !matches!(
            path.first(),
            Some(
                ProjectedMessage::Visible(Message {
                    role: MessageRole::System,
                    parent_message_id: None,
                    ..
                }) | ProjectedMessage::Redacted {
                    role: MessageRole::System,
                    parent_message_id: None,
                    ..
                }
            )
        ) {
            return Err(MessageServiceError::InvalidRoot);
        }
        Ok(path)
    }

    /// Projects the current root-to-head branch of a Session.
    ///
    /// # Errors
    ///
    /// Returns a [`MessageServiceError`] when the Session/path is missing or
    /// violates Project/tree invariants.
    pub fn session_view(&self, session_id: &SessionId) -> Result<SessionView, MessageServiceError> {
        let session = self.sessions.get_session(session_id)?;
        let messages = self.message_path(&session.current_message_id)?;
        let Some(head) = messages.last() else {
            return Err(MessageServiceError::InvalidRoot);
        };
        let head_project = projection_project(head);
        require_project(&session.project_id, head_project)?;
        Ok(SessionView { session, messages })
    }

    /// Appends a direct child, then advances a Session with optimistic locking.
    ///
    /// Message persistence happens first. The new Message remains addressable if
    /// the Session compare-and-swap conflicts or its store update fails.
    ///
    /// On a compare-and-swap race the returned error contains the current
    /// Session and the newly preserved sibling Message identity.
    ///
    /// # Errors
    ///
    /// Returns a [`MessageServiceError`] for invalid content/parent/project,
    /// persistence failure, or [`MessageServiceError::PointerConflict`].
    pub fn append_to_session(
        &self,
        session_id: &SessionId,
        expected_head: &MessageId,
        expected_version: u64,
        message: Message,
    ) -> Result<Session, MessageServiceError> {
        message.validate()?;
        if message.parent_message_id.as_ref() != Some(expected_head) {
            return Err(MessageServiceError::NotDirectChild);
        }
        let session = self.sessions.get_session(session_id)?;
        require_project(&session.project_id, &message.project_id)?;
        let parent = self.messages.get_message(expected_head)?;
        require_project(&message.project_id, &parent.message.project_id)?;

        let appended = self.append(message)?;
        let new_head = appended.id;
        let advance = self
            .sessions
            .advance_head(session_id, expected_head, expected_version, &new_head)
            .map_err(|source| MessageServiceError::SessionUpdateFailed {
                source,
                preserved_message_id: new_head,
            })?;
        match advance {
            SessionAdvance::Advanced(session) => Ok(session),
            SessionAdvance::Conflict { observed } => Err(MessageServiceError::PointerConflict {
                observed: Box::new(observed),
                preserved_message_id: new_head,
            }),
        }
    }

    /// Appends a same-role sibling that replaces visible content without
    /// mutating the edited Message or its descendants.
    ///
    /// Call [`Self::open_session`] at the returned Message to view or continue
    /// the new branch.
    ///
    /// # Errors
    ///
    /// Returns [`MessageServiceError::InvalidEditFork`] unless both Messages
    /// are non-root standard Messages with the same Project/parent/role and
    /// distinct identities. A store's initialized root is not editable here.
    pub fn fork_edit(
        &self,
        edited_message_id: &MessageId,
        replacement: Message,
    ) -> Result<Message, MessageServiceError> {
        let edited = self.messages.get_message(edited_message_id)?.message;
        if edited.kind != MessageKind::Standard
            || edited.parent_message_id.is_none()
            || replacement.kind != MessageKind::Standard
            || replacement.id == edited.id
            || replacement.project_id != edited.project_id
            || replacement.parent_message_id != edited.parent_message_id
            || replacement.role != edited.role
        {
            return Err(MessageServiceError::InvalidEditFork);
        }
        self.append(replacement)
    }

    /// Appends a new assistant sibling for regeneration without mutating the
    /// prior assistant Message.
    ///
    /// # Errors
    ///
    /// Returns [`MessageServiceError::InvalidRegenerationFork`] unless the old
    /// and new Messages are distinct standard assistant siblings in one Project.
    pub fn fork_regeneration(
        &self,
        prior_assistant_id: &MessageId,
        replacement: Message,
    ) -> Result<Message, MessageServiceError> {
        let prior = self.messages.get_message(prior_assistant_id)?.message;
        if prior.kind != MessageKind::Standard
            || prior.role != MessageRole::Assistant
            || replacement.kind != MessageKind::Standard
            || replacement.role != MessageRole::Assistant
            || replacement.id == prior.id
            || replacement.project_id != prior.project_id
            || replacement.parent_message_id != prior.parent_message_id
        {
            return Err(MessageServiceError::InvalidRegenerationFork);
        }
        self.append(replacement)
    }
}

fn require_project(expected: &ProjectId, actual: &ProjectId) -> Result<(), MessageServiceError> {
    if expected == actual {
        Ok(())
    } else {
        Err(MessageServiceError::ProjectMismatch {
            expected: expected.clone(),
            actual: actual.clone(),
        })
    }
}

fn projection_project(message: &ProjectedMessage) -> &ProjectId {
    match message {
        ProjectedMessage::Visible(message) => &message.project_id,
        ProjectedMessage::Redacted { project_id, .. } => project_id,
    }
}
