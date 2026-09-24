//! Transport-independent filesystem request handling.

pub mod checkout;
pub mod files;
pub mod forge;
pub mod github_projects;
pub mod workspace_recovery;
pub mod worktrees;

/// Safe host-facing dispatch failure; business failures remain in typed results.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ErrorCode {
    /// Invalid parameters or binary frame.
    #[error("Invalid message or parameters")]
    InvalidMessage,
    /// Unknown filesystem method.
    #[error("Unknown method")]
    MethodNotFound,
    /// Filesystem I/O or result encoding failed.
    #[error("Project I/O failed")]
    ProjectIo,
    /// Registry I/O or result encoding failed.
    #[error("Registry I/O failed")]
    RegistryIo,
    /// A connection-local resource limit was exceeded.
    #[error("Resource budget exhausted")]
    ResourceExhausted,
}
