//! Transport-independent Agent requests and failures.

pub mod agent_execution;
pub mod agent_runtime;
pub mod agents;

/// Stable Agent failures mapped by the transport protocol.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ErrorCode {
    /// Invalid Agent request.
    #[error("InvalidMessage")]
    InvalidMessage,
    /// Requested runtime capability is unavailable.
    #[error("UnsupportedCapability")]
    UnsupportedCapability,
    /// Unknown Agent method.
    #[error("MethodNotFound")]
    MethodNotFound,
    /// Agent persistence failed.
    #[error("AgentIo")]
    AgentIo,
    /// No Agent has this identity.
    #[error("AgentNotFound")]
    AgentNotFound,
    /// No such immutable revision.
    #[error("AgentRevisionNotFound")]
    AgentRevisionNotFound,
    /// The observed Agent revision has changed.
    #[error("AgentRevisionConflict")]
    AgentRevisionConflict,
    /// The default selection has changed.
    #[error("AgentDefaultConflict")]
    AgentDefaultConflict,
    /// The selected Agent is disabled.
    #[error("AgentDisabled")]
    AgentDisabled,
    /// The Agent is still the default.
    #[error("AgentIsDefault")]
    AgentIsDefault,
    /// The retry key was reused with different arguments.
    #[error("IdempotencyConflict")]
    IdempotencyConflict,
    /// The catalog is busy.
    #[error("CatalogBusy")]
    CatalogBusy,
    /// The persisted format is unsupported.
    #[error("UnsupportedFormat")]
    UnsupportedFormat,
    /// Workspace placement could not be read.
    #[error("RegistryIo")]
    RegistryIo,
}
