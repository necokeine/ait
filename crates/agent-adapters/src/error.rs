use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
/// Variants represented by `AdapterErrorKind`.
pub enum AdapterErrorKind {
    /// Selects the `InvalidConfiguration` variant.
    InvalidConfiguration,
    /// Selects the `ProcessSpawn` variant.
    ProcessSpawn,
    /// Selects the `ProcessExited` variant.
    ProcessExited,
    /// Selects the `Protocol` variant.
    Protocol,
    /// Selects the `Authentication` variant.
    Authentication,
    /// Selects the `RateLimited` variant.
    RateLimited,
    /// Selects the `Unavailable` variant.
    Unavailable,
    /// Selects the `Cancelled` variant.
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[error("{kind:?}: {message}")]
/// Data carried by `AdapterError`.
pub struct AdapterError {
    /// Kind value.
    pub kind: AdapterErrorKind,
    /// Message value.
    pub message: String,
    /// Retryable value.
    pub retryable: bool,
    /// Code value.
    pub code: Option<String>,
}

impl AdapterError {
    /// Creates an adapter error with no provider-specific code.
    pub fn new(kind: AdapterErrorKind, message: impl Into<String>, retryable: bool) -> Self {
        Self {
            kind,
            message: message.into(),
            retryable,
            code: None,
        }
    }

    /// Creates a non-retryable protocol error.
    pub fn protocol(message: impl Into<String>) -> Self {
        Self::new(AdapterErrorKind::Protocol, message, false)
    }

    #[must_use]
    /// Creates a non-retryable cancellation error.
    pub fn cancelled() -> Self {
        Self::new(
            AdapterErrorKind::Cancelled,
            "agent invocation cancelled",
            false,
        )
    }
}
