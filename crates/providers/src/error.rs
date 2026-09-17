use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
/// Stable classification for provider invocation failures.
pub enum ProviderErrorKind {
    /// The request is invalid.
    InvalidRequest,
    /// The provider lacks a required capability.
    CapabilityUnsupported,
    /// Authentication failed or credentials are unavailable.
    Authentication,
    /// The credential lacks permission for the operation.
    Permission,
    /// The provider rejected the request because of rate limits.
    RateLimited,
    /// The provider service is temporarily unavailable.
    Unavailable,
    /// The invocation exceeded its deadline.
    Timeout,
    /// The caller cancelled the invocation.
    Cancelled,
    /// The provider response violated the expected protocol.
    Protocol,
    /// An unclassified internal error occurred.
    Internal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "milliseconds", rename_all = "snake_case")]
/// Guidance for whether and when an invocation may be retried.
pub enum RetryDirective {
    /// The failure must not be retried automatically.
    Never,
    /// Retry using the caller's exponential backoff policy.
    Backoff,
    /// Retry after the specified number of milliseconds.
    AfterMillis(u64),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[error("{kind:?}: {message}")]
/// Provider-neutral error returned by an adapter.
pub struct ProviderError {
    /// Stable error category.
    pub kind: ProviderErrorKind,
    /// Human-readable diagnostic message.
    pub message: String,
    /// Retry guidance supplied by the adapter.
    pub retry: RetryDirective,
    /// Provider HTTP status, when the transport exposed one.
    pub http_status: Option<u16>,
    /// Provider-specific machine-readable error code, when available.
    pub provider_code: Option<String>,
}

impl ProviderError {
    /// Creates an error with no transport status or provider code.
    pub fn new(kind: ProviderErrorKind, message: impl Into<String>, retry: RetryDirective) -> Self {
        Self {
            kind,
            message: message.into(),
            retry,
            http_status: None,
            provider_code: None,
        }
    }

    /// Creates a non-retryable invalid-request error.
    pub fn invalid(message: impl Into<String>) -> Self {
        Self::new(
            ProviderErrorKind::InvalidRequest,
            message,
            RetryDirective::Never,
        )
    }

    /// Creates a non-retryable unsupported-capability error.
    pub fn unsupported(message: impl Into<String>) -> Self {
        Self::new(
            ProviderErrorKind::CapabilityUnsupported,
            message,
            RetryDirective::Never,
        )
    }

    #[must_use]
    /// Creates a non-retryable cancellation error.
    pub fn cancelled() -> Self {
        Self::new(
            ProviderErrorKind::Cancelled,
            "provider invocation cancelled",
            RetryDirective::Never,
        )
    }
}
