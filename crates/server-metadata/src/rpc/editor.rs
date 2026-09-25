//! Compatibility responses for editor operations moved to the desktop application.

use serde::Deserialize;
use serde_json::{Value, json};
use server_model::ErrorCode;

const MOVED: &str =
    "Editor opening moved to the desktop app and is no longer supported by the daemon";

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct OpenRequest {
    path: String,
    editor_id: String,
    mode: Option<Mode>,
    cwd: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum Mode {
    Open,
    Reveal,
}

/// Validate the legacy request and return Paseo's desktop migration response.
/// # Errors
/// Returns invalid-message for malformed parameters, or method-not-found for other methods.
pub fn execute(method: &str, params: Value) -> Result<Value, ErrorCode> {
    match method {
        "editor.available.list.request" if params.is_object() => {
            Ok(json!({"editors":[], "error":MOVED}))
        }
        "editor.available.list.request" => Err(ErrorCode::InvalidMessage),
        "editor.open.request" => {
            let request: OpenRequest =
                serde_json::from_value(params).map_err(|_| ErrorCode::InvalidMessage)?;
            if request.editor_id.trim().is_empty() {
                return Err(ErrorCode::InvalidMessage);
            }
            // The desktop owns these values; the daemon must not open any local application.
            let _ = (request.path, request.mode, request.cwd);
            Ok(json!({"error":MOVED}))
        }
        _ => Err(ErrorCode::MethodNotFound),
    }
}

#[cfg(test)]
mod tests;
