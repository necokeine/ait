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

impl From<crate::rpc::ErrorCode> for server_model::ErrorCode {
    fn from(error: crate::rpc::ErrorCode) -> Self {
        match error {
            crate::rpc::ErrorCode::InvalidMessage => Self::InvalidMessage,
            crate::rpc::ErrorCode::UnsupportedCapability => Self::UnsupportedCapability,
            crate::rpc::ErrorCode::MethodNotFound => Self::MethodNotFound,
            crate::rpc::ErrorCode::AgentIo => Self::AgentIo,
            crate::rpc::ErrorCode::AgentNotFound => Self::AgentNotFound,
            crate::rpc::ErrorCode::AgentRevisionNotFound => Self::AgentRevisionNotFound,
            crate::rpc::ErrorCode::AgentRevisionConflict => Self::AgentRevisionConflict,
            crate::rpc::ErrorCode::AgentDefaultConflict => Self::AgentDefaultConflict,
            crate::rpc::ErrorCode::AgentDisabled => Self::AgentDisabled,
            crate::rpc::ErrorCode::AgentIsDefault => Self::AgentIsDefault,
            crate::rpc::ErrorCode::IdempotencyConflict => Self::IdempotencyConflict,
            crate::rpc::ErrorCode::CatalogBusy => Self::CatalogBusy,
            crate::rpc::ErrorCode::UnsupportedFormat => Self::UnsupportedFormat,
            crate::rpc::ErrorCode::RegistryIo => Self::RegistryIo,
        }
    }
}
