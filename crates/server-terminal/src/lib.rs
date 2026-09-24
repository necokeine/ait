//! Independent PTY terminal protocol, application service, and local process adapter.

pub mod local;
pub mod ports;
pub mod protocol;
pub mod rpc;
mod screen;
pub mod service;

/// Stable terminal failures; diagnostics never include child output or inherited environment.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    /// Malformed input, unsupported dimensions, or an invalid path.
    #[error("Invalid terminal parameters")]
    Invalid,
    /// The terminal has exited or does not exist.
    #[error("Terminal not found")]
    NotFound,
    /// No active workspace matches the requested placement.
    #[error("Workspace is not active or does not exist")]
    WorkspaceNotFound,
    /// Registry access failed.
    #[error("Workspace registry operation failed")]
    Registry,
    /// A bounded process, output, or input budget was exceeded.
    #[error("Terminal resource budget exhausted")]
    Exhausted,
    /// Native PTY creation, communication, or cleanup failed.
    #[error("Terminal I/O failed")]
    Io,
    /// The method does not belong to this service.
    #[error("Unknown terminal method")]
    MethodNotFound,
}

#[cfg(test)]
mod test_support;
