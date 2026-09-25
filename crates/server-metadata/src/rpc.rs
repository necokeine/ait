//! Transport-independent metadata request handling and safe business failures.

pub mod daemon;
pub mod directory;
pub mod server;
pub mod workspace_labels;

/// Stable metadata failure mapped into the host's public RPC error envelope.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ErrorCode {
    /// Metadata invalid message failure.
    #[error("InvalidMessage")]
    InvalidMessage,
    /// Metadata unsupported capability failure.
    #[error("UnsupportedCapability")]
    UnsupportedCapability,
    /// Metadata method not found failure.
    #[error("MethodNotFound")]
    MethodNotFound,
    /// Metadata registry io failure.
    #[error("RegistryIo")]
    RegistryIo,
    /// Metadata daemon config invalid failure.
    #[error("DaemonConfigInvalid")]
    DaemonConfigInvalid,
    /// Metadata daemon io failure.
    #[error("DaemonIo")]
    DaemonIo,
    /// Metadata workspace not found failure.
    #[error("WorkspaceNotFound")]
    WorkspaceNotFound,
    /// Metadata label name empty failure.
    #[error("LabelNameEmpty")]
    LabelNameEmpty,
    /// Metadata label not found failure.
    #[error("LabelNotFound")]
    LabelNotFound,
    /// Metadata label name taken failure.
    #[error("LabelNameTaken")]
    LabelNameTaken,
    /// Metadata workspace label storage uncertain failure.
    #[error("WorkspaceLabelStorageUncertain")]
    WorkspaceLabelStorageUncertain,
}
pub mod workspace_automation;
pub mod workspace_state;

impl From<crate::rpc::ErrorCode> for server_model::ErrorCode {
    fn from(error: crate::rpc::ErrorCode) -> Self {
        match error {
            crate::rpc::ErrorCode::InvalidMessage => Self::InvalidMessage,
            crate::rpc::ErrorCode::UnsupportedCapability => Self::UnsupportedCapability,
            crate::rpc::ErrorCode::MethodNotFound => Self::MethodNotFound,
            crate::rpc::ErrorCode::RegistryIo => Self::RegistryIo,
            crate::rpc::ErrorCode::DaemonConfigInvalid => Self::DaemonConfigInvalid,
            crate::rpc::ErrorCode::DaemonIo => Self::DaemonIo,
            crate::rpc::ErrorCode::WorkspaceNotFound => Self::WorkspaceNotFound,
            crate::rpc::ErrorCode::LabelNameEmpty => Self::LabelNameEmpty,
            crate::rpc::ErrorCode::LabelNotFound => Self::LabelNotFound,
            crate::rpc::ErrorCode::LabelNameTaken => Self::LabelNameTaken,
            crate::rpc::ErrorCode::WorkspaceLabelStorageUncertain => {
                Self::WorkspaceLabelStorageUncertain
            }
        }
    }
}
