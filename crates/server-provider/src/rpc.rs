//! Transport-independent Agent requests and failures.

pub mod agent_execution;
pub mod agent_runtime;
pub mod agents;

/// Stable Agent failures mapped by the transport protocol.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ErrorCode {
    /// A request exceeded a budget or conflicted with an immutable receipt.
    #[error("ResourceExhausted")]
    ResourceExhausted,
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
            crate::rpc::ErrorCode::ResourceExhausted => Self::ResourceExhausted,
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

#[cfg(test)]
mod tests;

pub(crate) mod fork_context;
pub(crate) mod timeline;

impl From<server_model::ErrorCode> for ErrorCode {
    fn from(error: server_model::ErrorCode) -> Self {
        match error {
            server_model::ErrorCode::InvalidMessage => Self::InvalidMessage,
            server_model::ErrorCode::UnsupportedCapability => Self::UnsupportedCapability,
            server_model::ErrorCode::MethodNotFound => Self::MethodNotFound,
            server_model::ErrorCode::RegistryIo => Self::RegistryIo,
            server_model::ErrorCode::IdempotencyConflict => Self::IdempotencyConflict,
            server_model::ErrorCode::ResourceExhausted => Self::ResourceExhausted,
            server_model::ErrorCode::AgentNotFound => Self::AgentNotFound,
            server_model::ErrorCode::UnsupportedFormat => Self::UnsupportedFormat,
            server_model::ErrorCode::CatalogBusy => Self::CatalogBusy,
            _ => Self::AgentIo,
        }
    }
}
