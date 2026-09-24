use super::*;

fn hello() -> Hello {
    Hello {
        protocol: VersionOffer {
            major: 1,
            min_minor: 0,
            max_minor: 2,
        },
        client_id: "test-client".to_owned(),
        capabilities: vec!["server.info".to_owned(), "future.optional".to_owned()],
        required_capabilities: vec!["connection.ping".to_owned()],
    }
}

#[test]
fn negotiates_supported_intersection_and_rejects_required_unknown() {
    let mut offer = hello();
    assert_eq!(
        offer.negotiate().unwrap(),
        ["server.info", "connection.ping"]
    );
    offer.required_capabilities.push("run.submit".to_owned());
    assert_eq!(offer.negotiate(), Err(ErrorCode::UnsupportedCapability));
}

#[test]
fn rejects_incompatible_and_reversed_version_ranges() {
    for protocol in [
        VersionOffer {
            major: 2,
            min_minor: 0,
            max_minor: 0,
        },
        VersionOffer {
            major: 1,
            min_minor: 1,
            max_minor: 2,
        },
        VersionOffer {
            major: 1,
            min_minor: 2,
            max_minor: 1,
        },
    ] {
        let mut offer = hello();
        offer.protocol = protocol;
        assert_eq!(offer.negotiate(), Err(ErrorCode::IncompatibleVersion));
    }
}

#[test]
fn bounds_diagnostic_ids_and_capability_lists() {
    for value in [String::new(), "x".repeat(129), "bad\nlabel".to_owned()] {
        let mut offer = hello();
        offer.client_id = value;
        assert_eq!(offer.negotiate(), Err(ErrorCode::InvalidMessage));
    }
    for required in [false, true] {
        let mut offer = hello();
        if required {
            offer.required_capabilities = vec!["x".to_owned(); 65];
        } else {
            offer.capabilities = vec!["x".to_owned(); 65];
        }
        assert_eq!(offer.negotiate(), Err(ErrorCode::InvalidMessage));
    }
    let mut offer = hello();
    offer.capabilities.push(String::new());
    assert_eq!(offer.negotiate(), Err(ErrorCode::InvalidMessage));
    assert!(valid_id(&"x".repeat(128)));
}

#[test]
fn wire_round_trips_and_tolerates_optional_future_fields() {
    let legacy: ServerMessage = serde_json::from_value(serde_json::json!({
        "type":"error","request_id":"1","code":"method_not_found"
    }))
    .unwrap();
    assert!(
        matches!(legacy, ServerMessage::Error { message, retryable: false, .. } if message.is_empty())
    );
    let message = ClientMessage::Hello(hello());
    let mut value = serde_json::to_value(&message).unwrap();
    value["future_optional"] = serde_json::json!(true);
    assert_eq!(
        serde_json::from_value::<ClientMessage>(value).unwrap(),
        message
    );
    let request: ClientMessage = serde_json::from_value(serde_json::json!({
        "type":"request", "request_id":"1", "method":"server.info"
    }))
    .unwrap();
    assert!(matches!(
        request,
        ClientMessage::Request {
            params: Value::Null,
            ..
        }
    ));
    for message in [
        ServerMessage::Error {
            request_id: Some("1".to_owned()),
            code: ErrorCode::MethodNotFound,
            message: ErrorCode::MethodNotFound.message().to_owned(),
            retryable: false,
        },
        ServerMessage::Status {
            subscription_id: "2".to_owned(),
            lifecycle: Lifecycle::Draining,
        },
        ServerMessage::Response {
            request_id: "3".to_owned(),
            result: Value::Null,
        },
    ] {
        assert_eq!(
            serde_json::from_str::<ServerMessage>(&serde_json::to_string(&message).unwrap())
                .unwrap(),
            message
        );
    }
    let limits = Limits::default();
    assert_eq!(limits.message_bytes, 1_048_576);
    assert_eq!(limits.queue_messages, 256);
    assert_eq!(limits.queue_bytes, 4_194_304);
    assert_eq!(limits.connections, 64);
}

#[test]
fn placeholder_event_response_and_error_have_stable_wire_shapes() {
    let event: ClientMessage = serde_json::from_value(serde_json::json!({
        "type":"event", "method":"terminal.input"
    }))
    .unwrap();
    assert_eq!(
        event,
        ClientMessage::Event {
            method: "terminal.input".to_owned(),
            params: Value::Null,
        }
    );
    let response = ClientMessage::Response {
        request_id: Some("browser-1".to_owned()),
        method: "browser.automation.execute.response".to_owned(),
        params: serde_json::json!({"ok":true}),
    };
    assert_eq!(
        serde_json::from_value::<ClientMessage>(serde_json::to_value(&response).unwrap()).unwrap(),
        response
    );
    let code = ErrorCode::NotImplemented;
    assert_eq!(serde_json::to_value(code).unwrap(), "not_implemented");
    assert!(!code.retryable());
    assert_eq!(code.message(), "Method is not implemented yet");
}

#[test]
fn metadata_failures_keep_public_codes_and_retry_semantics() {
    use server_metadata::rpc::ErrorCode as MetadataError;

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

#[test]
fn request_shapes_match_paseo_and_keep_only_dotted_methods() {
    assert_eq!(server_filesystem::protocol::files::CAPABILITIES.len(), 11);
    assert!(
        server_filesystem::protocol::files::CAPABILITIES
            .iter()
            .all(|name| {
                crate::methods::PASEO_METHODS
                    .iter()
                    .any(|method| method.canonical_name == *name)
            })
    );
}

#[test]
fn skill_placeholders_are_owned_by_filesystem_and_remain_canonical_requests() {
    for method in server_filesystem::protocol::skills::METHODS {
        let spec = methods::PASEO_METHODS
            .iter()
            .find(|spec| spec.canonical_name == *method)
            .unwrap();
        assert_eq!(spec.group, methods::MethodGroup::Skills);
        assert_eq!(spec.kind, methods::InboundKind::Request);
    }
}

#[test]
fn provider_failures_keep_public_codes_messages_and_retry_semantics() {
    use server_provider::rpc::ErrorCode as ProviderError;

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
