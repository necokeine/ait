use server_model::ErrorCode;

#[test]
fn metadata_failures_keep_public_codes_and_retry_semantics() {
    use super::ErrorCode as MetadataError;

    for (business, code, retryable) in [
        (MetadataError::InvalidMessage, "invalid_message", false),
        (
            MetadataError::UnsupportedCapability,
            "unsupported_capability",
            false,
        ),
        (MetadataError::MethodNotFound, "method_not_found", false),
        (MetadataError::RegistryIo, "registry_io", true),
        (
            MetadataError::DaemonConfigInvalid,
            "daemon_config_invalid",
            false,
        ),
        (MetadataError::DaemonIo, "daemon_io", true),
        (
            MetadataError::WorkspaceNotFound,
            "workspace_not_found",
            false,
        ),
        (MetadataError::LabelNameEmpty, "label_name_empty", false),
        (MetadataError::LabelNotFound, "label_not_found", false),
        (MetadataError::LabelNameTaken, "label_name_taken", false),
        (
            MetadataError::WorkspaceLabelStorageUncertain,
            "workspace_label_storage_uncertain",
            true,
        ),
    ] {
        let error = ErrorCode::from(business);
        assert_eq!(serde_json::to_value(error).unwrap(), code);
        assert_eq!(error.retryable(), retryable);
    }
}
