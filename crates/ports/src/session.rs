use ait_domain::{MessageId, Session, SessionId};

/// Result of compare-and-swapping a Session head.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SessionAdvance {
    /// The Session now points at the supplied Message.
    Advanced(Session),
    /// The stale Session ref was left untouched.
    Conflict {
        /// Current Session state observed by the compare-and-swap.
        observed: Session,
    },
}

/// Stable failures exposed by Session persistence.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SessionStoreError {
    /// A Session identity is unknown.
    SessionNotFound(SessionId),
    /// A generated Session identity is already in use.
    IdentityConflict(SessionId),
    /// Adapter-specific failure with a safe diagnostic.
    Other(String),
}

impl std::fmt::Display for SessionStoreError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SessionNotFound(id) => write!(formatter, "session not found: {}", id.as_str()),
            Self::IdentityConflict(id) => {
                write!(
                    formatter,
                    "session identity already exists: {}",
                    id.as_str()
                )
            }
            Self::Other(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for SessionStoreError {}

/// Persistence boundary for movable Session refs.
pub trait SessionStore: Send + Sync {
    /// Creates a Session pointing at a Message already validated by the application layer.
    ///
    /// # Errors
    ///
    /// Returns [`SessionStoreError::IdentityConflict`] when the identity is
    /// already used or another adapter failure prevents creation.
    fn create_session(&self, session: Session) -> Result<Session, SessionStoreError>;

    /// Loads a Session.
    ///
    /// # Errors
    ///
    /// Returns [`SessionStoreError::SessionNotFound`] for an unknown identity.
    fn get_session(&self, id: &SessionId) -> Result<Session, SessionStoreError>;

    /// Compare-and-swaps the Session from `expected_head`/`expected_version` to
    /// an already appended `new_head` Message.
    ///
    /// A stale pointer is an expected [`SessionAdvance::Conflict`] outcome.
    ///
    /// # Errors
    ///
    /// Returns a [`SessionStoreError`] when the Session cannot be read or updated.
    fn advance_head(
        &self,
        session_id: &SessionId,
        expected_head: &MessageId,
        expected_version: u64,
        new_head: &MessageId,
    ) -> Result<SessionAdvance, SessionStoreError>;
}
