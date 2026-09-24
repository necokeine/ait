//! Provider-owned Agent sessions used by the independent server.
//!
//! These contracts mirror the create/resume, runtime inspection, persistence and close portion
//! of Paseo's `AgentClient` and `AgentSession`. A concrete provider adapter owns native execution.

use std::fmt::Debug;
use std::future::Future;
use std::pin::Pin;

use server_domain::agent_runtime::{
    AgentPersistenceHandle, StoredAgentConfig, StoredAgentRuntimeInfo,
};

/// A sendable provider operation borrowing its client or session.
pub type AgentSessionFuture<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, AgentSessionError>> + Send + 'a>>;

/// Provider failure safe to expose at the application boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum AgentSessionError {
    /// The provider cannot currently start or resume a session.
    #[error("provider is unavailable")]
    Unavailable,
    /// The provider failed an operation or returned inconsistent facts.
    #[error("provider session operation failed")]
    Failed,
    /// The provider rejected a request while the transport remains usable.
    #[error("provider rejected the session operation")]
    Rejected,
}

/// Provider-independent inputs required to construct or resume a native session.
#[derive(Debug, Clone, PartialEq)]
pub struct AgentSessionSpec {
    /// Provider identity.
    pub provider: String,
    /// Native session working directory.
    pub cwd: String,
    /// Persistable provider configuration.
    pub config: StoredAgentConfig,
}

/// Whether a resumed native session may accept a new foreground turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentResumePurpose {
    /// Resume an active Agent for interaction.
    Interactive,
    /// Read the history of an archived Agent without claiming an interactive writer.
    History,
}

/// Final native foreground-turn result. This is not a host Run or Message tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentTurnEvent {
    /// The native provider drained its turn successfully.
    Completed(Option<String>),
    /// The native provider acknowledged interruption.
    Cancelled,
    /// The native provider failed, exited, or requested unsupported interaction.
    Failed,
}

/// Live provider session. Closing releases resources without deleting native history.
pub trait AgentSession: Debug + Send {
    /// Return the provider that owns this session.
    fn provider(&self) -> &str;

    /// Inspect current provider facts after creation or resume.
    ///
    /// # Errors
    /// Returns a provider error when the native runtime cannot be inspected.
    fn runtime_info(&mut self) -> AgentSessionFuture<'_, StoredAgentRuntimeInfo>;

    /// Describe the native identity required to resume this session, if available.
    fn persistence(&self) -> Option<AgentPersistenceHandle>;

    /// Start one text-only foreground turn and return its native identity.
    ///
    /// # Errors
    /// Returns an error for a busy, history-only, or unsupported session.
    fn start_turn<'a>(&'a mut self, _text: &'a str) -> AgentSessionFuture<'a, String> {
        Box::pin(async { Err(AgentSessionError::Unavailable) })
    }

    /// Interrupt the identified native turn without deleting session history.
    ///
    /// # Errors
    /// Returns an error when the provider cannot acknowledge interruption.
    fn cancel_turn<'a>(&'a mut self, _turn_id: &'a str) -> AgentSessionFuture<'a, ()> {
        Box::pin(async { Err(AgentSessionError::Unavailable) })
    }

    /// Read a queued terminal event without waiting for new provider output.
    ///
    /// # Errors
    /// Returns an error if the provider connection has failed.
    fn poll_turn(&mut self) -> Result<Option<AgentTurnEvent>, AgentSessionError> {
        Ok(None)
    }

    /// Release the live native runtime without deleting its durable history.
    ///
    /// # Errors
    /// Returns a provider error if runtime ownership remains uncertain.
    fn close(&mut self) -> AgentSessionFuture<'_, ()>;
}

/// Factory and availability boundary for one independent provider adapter.
pub trait AgentClient: Debug + Send + Sync {
    /// Return the provider identity served by this client.
    fn provider(&self) -> &str;

    /// Check whether the provider can launch sessions now.
    ///
    /// # Errors
    /// Returns a provider error if availability cannot be determined.
    fn is_available(&self) -> AgentSessionFuture<'_, bool>;

    /// Create a new native session using `spec`.
    ///
    /// # Errors
    /// Returns a provider error when construction fails.
    fn create_session<'a>(
        &'a self,
        spec: &'a AgentSessionSpec,
    ) -> AgentSessionFuture<'a, Box<dyn AgentSession>>;

    /// Resume the native session identified by `handle` for `purpose`.
    ///
    /// # Errors
    /// Returns a provider error when resume fails or the handle is no longer valid.
    fn resume_session<'a>(
        &'a self,
        handle: &'a AgentPersistenceHandle,
        spec: &'a AgentSessionSpec,
        purpose: AgentResumePurpose,
    ) -> AgentSessionFuture<'a, Box<dyn AgentSession>>;
}
