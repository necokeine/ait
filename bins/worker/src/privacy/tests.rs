use super::*;
#[test]
fn native_arguments_and_command_credentials_are_removed_before_projection() {
    let mut value = serde_json::json!({
        "arguments": {"private_key": "native-secret"},
        "command": "curl -H 'Authorization: Bearer native-secret'",
        "result": {"credential": "native-secret"},
    });
    assert!(ait_contracts::sensitive::sensitive_argument_reason(&value).is_some());
    redact_display(&mut value);
    assert!(!value.to_string().contains("native-secret"));
    assert!(
        ait_contracts::sensitive::sensitive_argument_reason(
            &serde_json::json!({"file_path":"test.py","content":"print('hello')"})
        )
        .is_none()
    );
}
