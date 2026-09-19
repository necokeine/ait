use super::*;
use serde_json::json;

#[test]
fn daemon_accepts_future_worker_minor_and_ignores_optional_fields() {
    let value = json!({
        "protocol_major": PROTOCOL_MAJOR,
        "protocol_minor": 4,
        "sequence": 1,
        "lease": null,
        "future_envelope_hint": "optional",
        "payload": {
            "type": "hello",
            "protocol_major": PROTOCOL_MAJOR,
            "protocol_minor": 4,
            "minimum_protocol_minor": 0,
            "capabilities": [
                "run-store-v1",
                "commit-ack-v1",
                "lease-v1",
                "tool-grants-v1",
                "native-codex-v1", "tool-interactions-v1",
                "future-optional-v1"
            ],
            "required_capabilities": [
                "run-store-v1",
                "commit-ack-v1",
                "lease-v1",
                "tool-grants-v1",
                "native-codex-v1", "tool-interactions-v1"
            ],
            "max_frame_bytes": 2_097_152,
            "pid": 42,
            "future_hello_hint": true
        }
    });
    let decoded: Envelope = serde_json::from_value(value).unwrap();
    let Payload::Hello(hello) = &decoded.payload else {
        panic!("expected hello")
    };
    let selected = hello.negotiate(MAX_FRAME_BYTES).unwrap();
    assert_eq!(selected.protocol_minor, PROTOCOL_MINOR);
    assert_eq!(selected.max_frame_bytes, MAX_FRAME_BYTES);
    assert_eq!(selected.capabilities.len(), REQUIRED_CAPABILITIES.len());
    let encoded = serde_json::to_value(decoded).unwrap();
    assert_eq!(encoded["protocol_minor"], 4);
    assert_eq!(encoded["payload"]["protocol_minor"], 4);
    assert_eq!(encoded["payload"]["minimum_protocol_minor"], 0);
    assert_eq!(encoded["payload"]["max_frame_bytes"], 2_097_152);
}

#[test]
fn worker_accepts_older_daemon_minor_and_ignores_optional_fields() {
    let value = json!({
        "protocol_major": PROTOCOL_MAJOR,
        "protocol_minor": 0,
        "sequence": 1,
        "lease": null,
        "future_envelope_hint": "optional",
        "payload": {
            "type": "hello_ack",
            "protocol_major": PROTOCOL_MAJOR,
            "protocol_minor": 0,
            "max_frame_bytes": 262_144,
            "capabilities": [
                "run-store-v1",
                "commit-ack-v1",
                "lease-v1",
                "tool-grants-v1",
                "native-codex-v1", "tool-interactions-v1"
            ],
            "future_ack_hint": true
        }
    });
    let decoded: Envelope = serde_json::from_value(value).unwrap();
    let Payload::HelloAck(ack) = &decoded.payload else {
        panic!("expected hello_ack")
    };
    ack.validate(&Hello::current()).unwrap();
    let encoded = serde_json::to_value(decoded).unwrap();
    assert_eq!(encoded["protocol_minor"], 0);
    assert_eq!(encoded["payload"]["protocol_minor"], 0);
    assert_eq!(encoded["payload"]["max_frame_bytes"], 262_144);
    assert_eq!(
        encoded["payload"]["capabilities"].as_array().unwrap().len(),
        REQUIRED_CAPABILITIES.len()
    );
}

#[test]
fn unknown_message_kind_and_required_capability_remain_fatal() {
    let unknown = json!({
        "protocol_major": PROTOCOL_MAJOR,
        "protocol_minor": 0,
        "sequence": 1,
        "lease": null,
        "payload": {"type": "future_required_message"}
    });
    assert!(serde_json::from_value::<Envelope>(unknown).is_err());

    let mut hello = Hello::current();
    hello
        .required_capabilities
        .push("unknown-required-v9".into());
    assert_eq!(hello.validate(), Err(ProtocolError::UnsupportedCapability));

    let incompatible_minor = Hello {
        protocol_minor: PROTOCOL_MINOR + 1,
        minimum_protocol_minor: PROTOCOL_MINOR + 1,
        ..Hello::current()
    };
    assert_eq!(
        incompatible_minor.negotiate(MAX_FRAME_BYTES),
        Err(ProtocolError::VersionMismatch)
    );

    let missing_capability = HelloAck {
        protocol_major: PROTOCOL_MAJOR,
        protocol_minor: PROTOCOL_MINOR,
        max_frame_bytes: MAX_FRAME_BYTES,
        capabilities: vec!["run-store-v1".into()],
    };
    assert_eq!(
        missing_capability.validate(&Hello::current()),
        Err(ProtocolError::UnsupportedCapability)
    );
}
