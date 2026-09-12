//! Auditable, fail-closed policy for tool arguments crossing a persistence boundary.

use serde_json::Value;

/// Raw tool arguments above this bound are rejected before a Message is committed.
pub const MAX_PRIVATE_TOOL_ARGUMENT_BYTES: usize = 16_384;

/// Stable policy reason. It deliberately carries no peer-controlled text.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SensitiveArgumentReason {
    InvalidJson,
    Oversized,
    SensitiveField,
    CredentialMarker,
    UriUserInfo,
    PemPrivateKey,
    AccessKeyPattern,
}

/// Parse and classify serialized tool arguments without echoing their contents.
///
/// # Errors
/// Returns a stable reason for malformed, oversized, or sensitive input.
pub fn validate_serialized_tool_arguments(
    serialized: &str,
) -> Result<Value, SensitiveArgumentReason> {
    if serialized.len() > MAX_PRIVATE_TOOL_ARGUMENT_BYTES {
        return Err(SensitiveArgumentReason::Oversized);
    }
    let value =
        serde_json::from_str(serialized).map_err(|_| SensitiveArgumentReason::InvalidJson)?;
    if let Some(reason) = sensitive_argument_reason(&value) {
        return Err(reason);
    }
    Ok(value)
}

/// Return the first stable reason a JSON value is unsafe to persist as tool input.
#[must_use]
pub fn sensitive_argument_reason(value: &Value) -> Option<SensitiveArgumentReason> {
    match value {
        Value::Object(fields) => fields.iter().find_map(|(key, value)| {
            sensitive_key(key)
                .then_some(SensitiveArgumentReason::SensitiveField)
                .or_else(|| sensitive_argument_reason(value))
        }),
        Value::Array(values) => values.iter().find_map(sensitive_argument_reason),
        Value::String(text) => sensitive_text_reason(text),
        Value::Null | Value::Bool(_) | Value::Number(_) => None,
    }
}

/// Redact display-only JSON using the same classification policy as persistence.
pub fn redact_sensitive_display(value: &mut Value) {
    match value {
        Value::Object(fields) => {
            for (key, value) in fields {
                if sensitive_key(key) {
                    *value = Value::String("[REDACTED]".into());
                } else {
                    redact_sensitive_display(value);
                }
            }
        }
        Value::Array(values) => {
            for value in values {
                redact_sensitive_display(value);
            }
        }
        Value::String(text) if sensitive_text_reason(text).is_some() => {
            *text = "[REDACTED]".into();
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
}

fn sensitive_key(key: &str) -> bool {
    let normalized = key
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .flat_map(char::to_lowercase)
        .collect::<String>();
    matches!(
        normalized.as_str(),
        "apikey"
            | "token"
            | "accesstoken"
            | "refreshtoken"
            | "password"
            | "passphrase"
            | "secret"
            | "clientsecret"
            | "auth"
            | "authentication"
            | "authorization"
            | "authheader"
            | "authtoken"
            | "oauth"
            | "cookie"
            | "setcookie"
            | "sessioncookie"
    ) || normalized.contains("privatekey")
        || normalized.contains("accesskey")
        || normalized.contains("credential")
        || normalized.starts_with("authentication")
        || normalized.starts_with("authorization")
        || normalized.starts_with("oauth")
        || matches!(
            normalized.as_str(),
            "authdata" | "authinfo" | "authkey" | "authsecret"
        )
}

fn sensitive_text_reason(text: &str) -> Option<SensitiveArgumentReason> {
    let lower = text.to_ascii_lowercase();
    if lower.contains("-----begin ") && lower.contains("private key-----") {
        return Some(SensitiveArgumentReason::PemPrivateKey);
    }
    if uri_has_user_info(text) {
        return Some(SensitiveArgumentReason::UriUserInfo);
    }
    if [
        "bearer ",
        "authorization:",
        "authorization=",
        "authentication=",
        "api_key=",
        "api-key=",
        "access_key=",
        "access-key=",
        "access_token=",
        "refresh_token=",
        "aws_access_key_id=",
        "private_key=",
        "private-key=",
        "credential=",
        "credentials=",
        "auth=",
        "oauth_token=",
        "client_secret=",
        "--api-key ",
        "--access-key ",
        "--token ",
        "--password ",
        "--credential ",
        "password=",
        "passphrase=",
        "sk-proj-",
    ]
    .iter()
    .any(|marker| lower.contains(marker))
    {
        return Some(SensitiveArgumentReason::CredentialMarker);
    }
    if text
        .split(|character: char| !character.is_ascii_alphanumeric())
        .any(looks_like_access_key)
    {
        return Some(SensitiveArgumentReason::AccessKeyPattern);
    }
    None
}

fn uri_has_user_info(text: &str) -> bool {
    let Some((_, remainder)) = text.split_once("://") else {
        return false;
    };
    let authority = remainder.split(['/', '?', '#']).next().unwrap_or_default();
    authority
        .rsplit_once('@')
        .is_some_and(|(user_info, host)| !user_info.is_empty() && !host.is_empty())
}

fn looks_like_access_key(value: &str) -> bool {
    value.len() >= 16
        && ["AKIA", "ASIA", "AIDA", "AROA", "AIPA", "ANPA", "ANVA"]
            .iter()
            .any(|prefix| value.starts_with(prefix))
        && value
            .chars()
            .all(|character| character.is_ascii_uppercase() || character.is_ascii_digit())
}

#[cfg(test)]
mod tests {
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
}
