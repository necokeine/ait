//! Stable control error conversion helpers.
use ait_contracts::ApiError;
use ait_domain::{DomainError, ErrorCode};
use ait_ports::ControlStoreError;

pub(in crate::control) fn recovery_error(message: &str) -> ApiError {
    error(ErrorCode::RunRecoveryFailed, message, false)
}

pub(in crate::control) fn api_domain_error(error: ApiError) -> DomainError {
    DomainError {
        code: error.code,
        message: error.message,
        retryable: error.retryable,
        details: None,
        cause_id: None,
    }
}

pub(in crate::control) fn error(
    code: ErrorCode,
    message: impl Into<String>,
    retryable: bool,
) -> ApiError {
    ApiError {
        code,
        message: message.into(),
        retryable,
    }
}

#[allow(clippy::needless_pass_by_value)]
pub(in crate::control) fn store_error(failure: ControlStoreError) -> ApiError {
    error(ErrorCode::RunRecoveryFailed, failure.to_string(), true)
}

#[allow(clippy::needless_pass_by_value)]
pub(in crate::control) fn serialization_error(failure: serde_json::Error) -> ApiError {
    error(ErrorCode::RunRecoveryFailed, failure.to_string(), false)
}
