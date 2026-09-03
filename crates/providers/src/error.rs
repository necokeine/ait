use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderErrorKind {
    InvalidRequest,
    CapabilityUnsupported,
    Authentication,
    Permission,
    RateLimited,
    Unavailable,
    Timeout,
    Cancelled,
    Protocol,
    Internal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "milliseconds", rename_all = "snake_case")]
pub enum RetryDirective {
    Never,
    Backoff,
    AfterMillis(u64),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[error("{kind:?}: {message}")]
pub struct ProviderError {
    pub kind: ProviderErrorKind,
    pub message: String,
    pub retry: RetryDirective,
    pub http_status: Option<u16>,
    pub provider_code: Option<String>,
}

impl ProviderError {
    pub fn new(kind: ProviderErrorKind, message: impl Into<String>, retry: RetryDirective) -> Self {
        Self {
            kind,
            message: message.into(),
            retry,
            http_status: None,
            provider_code: None,
        }
    }

    pub fn invalid(message: impl Into<String>) -> Self {
        Self::new(
            ProviderErrorKind::InvalidRequest,
            message,
            RetryDirective::Never,
        )
    }

    pub fn unsupported(message: impl Into<String>) -> Self {
        Self::new(
            ProviderErrorKind::CapabilityUnsupported,
            message,
            RetryDirective::Never,
        )
    }

    #[must_use]
    pub fn cancelled() -> Self {
        Self::new(
            ProviderErrorKind::Cancelled,
            "provider invocation cancelled",
            RetryDirective::Never,
        )
    }
}
