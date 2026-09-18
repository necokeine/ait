use super::*;
use serde_json::json;

#[test]
fn common_sensitive_shapes_are_rejected_with_stable_reasons() {
    for (value, expected) in [
        (
            json!({"private_key": "material"}),
            SensitiveArgumentReason::SensitiveField,
        ),
        (
            json!({"aws_access_key_id": "material"}),
            SensitiveArgumentReason::SensitiveField,
        ),
        (
            json!({"credential": "material"}),
            SensitiveArgumentReason::SensitiveField,
        ),
        (
            json!({"auth": "material"}),
            SensitiveArgumentReason::SensitiveField,
        ),
        (
            json!({"authentication_token": "material"}),
            SensitiveArgumentReason::SensitiveField,
        ),
        (
            json!({"url": "https://user:password@example.test/path"}),
            SensitiveArgumentReason::UriUserInfo,
        ),
        (
            json!({"content": "-----BEGIN PRIVATE KEY-----\nmaterial\n-----END PRIVATE KEY-----"}),
            SensitiveArgumentReason::PemPrivateKey,
        ),
        (
            json!({"value": "AKIAIOSFODNN7EXAMPLE"}),
            SensitiveArgumentReason::AccessKeyPattern,
        ),
    ] {
        assert_eq!(sensitive_argument_reason(&value), Some(expected));
    }
    assert_eq!(
        sensitive_argument_reason(
            &json!({"file_path": "auth.rs", "content": "public information"})
        ),
        None
    );
}

#[test]
fn display_redaction_uses_the_persistence_policy() {
    let mut value = json!({
        "arguments": {
            "private_key": "private-material",
            "endpoint": "https://user:password@example.test/path"
        }
    });
    redact_sensitive_display(&mut value);
    let rendered = value.to_string();
    assert!(!rendered.contains("private-material"));
    assert!(!rendered.contains("password"));
}

#[test]
fn malformed_and_oversized_arguments_fail_closed_at_the_byte_boundary() {
    let malformed_secret = "NEC248_MALFORMED_UNIT_SECRET";
    let malformed = format!(r#"{{"file_path":"leak.txt","content":"{malformed_secret}""#);
    let malformed_error = validate_serialized_tool_arguments(&malformed).unwrap_err();
    assert_eq!(malformed_error, SensitiveArgumentReason::InvalidJson);
    assert!(!format!("{malformed_error:?}").contains(malformed_secret));

    let at_limit = format!("\"{}\"", "x".repeat(MAX_PRIVATE_TOOL_ARGUMENT_BYTES - 2));
    assert_eq!(at_limit.len(), MAX_PRIVATE_TOOL_ARGUMENT_BYTES);
    assert!(validate_serialized_tool_arguments(&at_limit).is_ok());

    let oversized = format!("{at_limit}x");
    assert_eq!(
        validate_serialized_tool_arguments(&oversized),
        Err(SensitiveArgumentReason::Oversized)
    );
    assert_eq!(
        validate_tool_argument_value(&json!({
            "content": "x".repeat(MAX_PRIVATE_TOOL_ARGUMENT_BYTES)
        })),
        Err(SensitiveArgumentReason::Oversized)
    );
}
