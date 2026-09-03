use ait_domain::{Message, MessageId, ProjectId, Session, SessionId, StoredMessage};

/// Result of atomically appending a Message and advancing a Session ref.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SessionAdvance {
    /// The Message was inserted and the Session ref advanced.
    Advanced(Session),
    /// The Message was inserted but the stale Session ref was left untouched.
    Conflict {
        /// Current Session state observed by the compare-and-swap.
        observed: Session,
        /// Newly inserted Message retained as a recoverable sibling branch.
        preserved_message_id: MessageId,
    },
}

/// Stable failures exposed by Message/Session persistence.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MessageStoreError {
    /// A Message identity is unknown.
    MessageNotFound(MessageId),
    /// A Session identity is unknown.
    SessionNotFound(SessionId),
    /// A supplied Message belongs to another Project.
    MessageProjectMismatch {
        /// Project required by the operation.
        expected: ProjectId,
        /// Project found on the Message.
        actual: ProjectId,
    },
    /// A generated identity is already in use.
    IdentityConflict(String),
    /// Adapter-specific failure with a safe diagnostic.
    Other(String),
}

impl std::fmt::Display for MessageStoreError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MessageNotFound(id) => write!(formatter, "message not found: {id}"),
            Self::SessionNotFound(id) => write!(formatter, "session not found: {}", id.as_str()),
            Self::MessageProjectMismatch { expected, actual } => write!(
                formatter,
                "message project mismatch: expected {}, got {}",
                expected.as_str(),
                actual.as_str()
            ),
            Self::IdentityConflict(id) => write!(formatter, "identity already exists: {id}"),
            Self::Other(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for MessageStoreError {}

/// Persistence boundary for the append-only Message forest and movable Session refs.
pub trait MessageStore: Send + Sync {
    /// Inserts an immutable root System Message.
    ///
    /// # Errors
    ///
    /// Returns [`MessageStoreError::IdentityConflict`] when the identity is
    /// already used or another adapter failure prevents insertion.
    fn insert_root(&self, root: Message) -> Result<Message, MessageStoreError>;

    /// Inserts an immutable non-root Message after rechecking the parent in the
    /// same persistence boundary.
    ///
    /// # Errors
    ///
    /// Returns a [`MessageStoreError`] when the parent is missing, belongs to a
    /// different Project, the identity conflicts, or insertion fails.
    fn append_message(&self, message: Message) -> Result<Message, MessageStoreError>;

    /// Loads a Message together with its independent redaction state.
    ///
    /// # Errors
    ///
    /// Returns [`MessageStoreError::MessageNotFound`] for an unknown identity.
    fn get_message(&self, id: &MessageId) -> Result<StoredMessage, MessageStoreError>;

    /// Creates a Session pointing at an existing Message.
    ///
    /// # Errors
    ///
    /// Returns a [`MessageStoreError`] when the identity conflicts or the target
    /// Message cannot be used.
    fn create_session(&self, session: Session) -> Result<Session, MessageStoreError>;

    /// Loads a Session.
    ///
    /// # Errors
    ///
    /// Returns [`MessageStoreError::SessionNotFound`] for an unknown identity.
    fn get_session(&self, id: &SessionId) -> Result<Session, MessageStoreError>;

    /// Atomically inserts `message` and compare-and-swaps a Session from
    /// `expected_head`/`expected_version` to that direct child.
    ///
    /// A pointer conflict is an expected outcome, not an adapter error. The
    /// insert must commit before returning [`SessionAdvance::Conflict`], so
    /// concurrent input is preserved as a sibling branch.
    ///
    /// # Errors
    ///
    /// Returns a [`MessageStoreError`] when the Message cannot be inserted or
    /// the Session is unavailable.
    fn append_and_advance(
        &self,
        session_id: &SessionId,
        expected_head: &MessageId,
        expected_version: u64,
        message: Message,
    ) -> Result<SessionAdvance, MessageStoreError>;
}
