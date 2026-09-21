use super::*;

#[test]
fn errors_preserve_actionable_semantics_without_input() {
    for (source, code) in [
        (AgentError::Invalid, ErrorCode::InvalidMessage),
        (AgentError::NotFound, ErrorCode::AgentNotFound),
        (
            AgentError::RevisionNotFound,
            ErrorCode::AgentRevisionNotFound,
        ),
        (
            AgentError::RevisionConflict,
            ErrorCode::AgentRevisionConflict,
        ),
        (AgentError::DefaultConflict, ErrorCode::AgentDefaultConflict),
        (AgentError::Disabled, ErrorCode::AgentDisabled),
        (AgentError::IsDefault, ErrorCode::AgentIsDefault),
        (
            AgentError::IdempotencyConflict,
            ErrorCode::IdempotencyConflict,
        ),
        (AgentError::Busy, ErrorCode::CatalogBusy),
        (AgentError::UnsupportedFormat, ErrorCode::UnsupportedFormat),
        (AgentError::Io, ErrorCode::AgentIo),
    ] {
        assert_eq!(error(source), code);
        assert!(!code.message().is_empty());
        assert_eq!(
            code.retryable(),
            matches!(source, AgentError::Busy | AgentError::Io)
        );
    }
}
