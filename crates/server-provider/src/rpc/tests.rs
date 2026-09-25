use server_model::ErrorCode;

#[test]
fn provider_failures_keep_public_codes_messages_and_retry_semantics() {
    use super::ErrorCode as ProviderError;

    for (business, expected, retryable) in [
        (ProviderError::InvalidMessage, "invalid_message", false),
        (
            ProviderError::UnsupportedCapability,
            "unsupported_capability",
            false,
        ),
        (ProviderError::MethodNotFound, "method_not_found", false),
        (ProviderError::AgentIo, "agent_io", true),
        (ProviderError::AgentNotFound, "agent_not_found", false),
        (
            ProviderError::AgentRevisionNotFound,
            "agent_revision_not_found",
            false,
        ),
        (
            ProviderError::AgentRevisionConflict,
            "agent_revision_conflict",
            false,
        ),
        (
            ProviderError::AgentDefaultConflict,
            "agent_default_conflict",
            false,
        ),
        (ProviderError::AgentDisabled, "agent_disabled", false),
        (ProviderError::AgentIsDefault, "agent_is_default", false),
        (
            ProviderError::IdempotencyConflict,
            "idempotency_conflict",
            false,
        ),
        (ProviderError::CatalogBusy, "catalog_busy", true),
        (
            ProviderError::UnsupportedFormat,
            "unsupported_format",
            false,
        ),
        (ProviderError::RegistryIo, "registry_io", true),
    ] {
        let error = ErrorCode::from(business);
        assert_eq!(serde_json::to_value(error).unwrap(), expected);
        assert!(!error.message().is_empty());
        assert_eq!(error.retryable(), retryable);
    }
}
