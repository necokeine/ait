use ait_domain::{Message, MessageId, ProjectId, StoredMessage};

/// Stable failures exposed by immutable Message persistence.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MessageStoreError {
    /// A Message identity is unknown.
    MessageNotFound(MessageId),
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

/// Persistence boundary for one initialized append-only Message forest.
///
/// A concrete store is initialized together with its root System Message; root
/// creation is deliberately absent from this steady-state interface. Session
/// refs are persisted through [`crate::SessionStore`], not here.
pub trait MessageStore: Send + Sync {
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
}
