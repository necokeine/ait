//! Fail-closed handling for credential fields and display-only native metadata.
use serde_json::Value;

fn credential_key(key: &str) -> bool {
    matches!(
        key.to_ascii_lowercase().replace(['-', '_'], "").as_str(),
        "apikey"
            | "token"
            | "accesstoken"
            | "refreshtoken"
            | "password"
            | "secret"
            | "clientsecret"
            | "authorization"
            | "cookie"
            | "setcookie"
    )
}
fn credential_text(value: &str) -> bool {
    let value = value.to_ascii_lowercase();
    [
        "bearer ",
        "authorization:",
        "authorization=",
        "api_key=",
        "api-key=",
        "--api-key ",
        "--token ",
        "--password ",
        "password=",
        "client_secret=",
        "sk-proj-",
    ]
    .iter()
    .any(|marker| value.contains(marker))
}
pub(crate) fn unsafe_arguments(value: &Value) -> bool {
    match value {
        Value::Object(fields) => fields
            .iter()
            .any(|(key, value)| credential_key(key) || unsafe_arguments(value)),
        Value::Array(values) => values.iter().any(unsafe_arguments),
        Value::String(value) => credential_text(value),
        _ => false,
    }
}
pub(crate) fn redact_display(value: &mut Value) {
    match value {
        Value::Object(fields) => {
            for (key, value) in fields {
                if credential_key(key) {
                    *value = Value::String("[REDACTED]".into());
                } else {
                    redact_display(value);
                }
            }
        }
        Value::Array(values) => {
            for value in values {
                redact_display(value);
            }
        }
        Value::String(text) if credential_text(text) => *text = "[REDACTED]".into(),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_arguments_and_command_credentials_are_removed_before_projection() {
        let mut value = serde_json::json!({"arguments":{"token":"native-secret"},"command":"curl -H 'Authorization: Bearer native-secret'","result":{"password":"native-secret"}});
        assert!(unsafe_arguments(&value));
        redact_display(&mut value);
        assert!(!value.to_string().contains("native-secret"));
        assert!(!unsafe_arguments(
            &serde_json::json!({"file_path":"test.py","content":"print('hello')"})
        ));
    }
}
