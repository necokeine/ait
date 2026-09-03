use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdapterErrorKind {
    InvalidConfiguration,
    ProcessSpawn,
    ProcessExited,
    Protocol,
    Authentication,
    RateLimited,
    Unavailable,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[error("{kind:?}: {message}")]
pub struct AdapterError {
    pub kind: AdapterErrorKind,
    pub message: String,
    pub retryable: bool,
    pub code: Option<String>,
}

impl AdapterError {
    pub fn new(kind: AdapterErrorKind, message: impl Into<String>, retryable: bool) -> Self {
        Self {
            kind,
            message: message.into(),
            retryable,
            code: None,
        }
    }

    pub fn protocol(message: impl Into<String>) -> Self {
        Self::new(AdapterErrorKind::Protocol, message, false)
    }

    #[must_use]
    pub fn cancelled() -> Self {
        Self::new(
            AdapterErrorKind::Cancelled,
            "agent invocation cancelled",
            false,
        )
    }
}
