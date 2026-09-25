//! Browser host wire contracts and bounded validation.
use serde::Deserialize;
use serde_json::{Value, json};
use server_model::ErrorCode;

/// Client operations owned by this capability.
pub const CAPABILITIES: &[&str] = &[
    "browser.host.register.request",
    "browser.automation.execute.response",
];
/// Commands supported by the upstream browser protocol.
pub const COMMANDS: &[&str] = &[
    "list_tabs",
    "new_tab",
    "snapshot",
    "click",
    "fill",
    "wait",
    "type",
    "keypress",
    "navigate",
    "back",
    "forward",
    "reload",
    "screenshot",
    "upload",
    "select",
    "hover",
    "drag",
    "logs",
    "evaluate",
    "scroll",
    "resize",
    "close_tab",
];
/// Explicit browser-host registration.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Register {
    /// Client implementation kind, used for diagnostics only.
    pub host_kind: String,
    /// Commands implemented by this host.
    pub supported_commands: Vec<String>,
}
/// Return whether a tab identity has the upstream UUID-v4 or legacy desktop shape.
#[must_use]
pub fn browser_id(value: &str) -> bool {
    if value.len() == 36
        && uuid::Uuid::parse_str(value)
            .is_ok_and(|id| id.get_version_num() == 4 && id.get_variant() == uuid::Variant::RFC4122)
    {
        return true;
    }
    value.split_once('-').is_some_and(|(time, id)| {
        time.len() >= 13
            && time.bytes().all(|b| b.is_ascii_digit())
            && !id.is_empty()
            && id.bytes().all(|b| b.is_ascii_hexdigit())
    })
}
fn text(value: &Value, key: &str) -> bool {
    value[key].as_str().is_some_and(|s| !s.is_empty())
}
fn reference(value: &Value, key: &str) -> bool {
    value[key].as_str().is_some_and(|s| {
        s.strip_prefix("@e")
            .is_some_and(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
    })
}
fn positive(value: &Value, key: &str) -> bool {
    value[key].as_u64().is_some_and(|v| v > 0)
}
fn url(value: &Value, key: &str) -> bool {
    value[key]
        .as_str()
        .and_then(|s| url::Url::parse(s).ok())
        .is_some_and(|u| matches!(u.scheme(), "http" | "https") && u.host_str().is_some())
}
/// Validate a server-originated command and add upstream defaults.
/// # Errors
/// Returns invalid-message for malformed arguments or unknown commands.
pub fn command(mut value: Value) -> Result<Value, ErrorCode> {
    let name = value["command"]
        .as_str()
        .filter(|s| COMMANDS.contains(s))
        .ok_or(ErrorCode::InvalidMessage)?
        .to_owned();
    if value.get("args").is_none() && ["list_tabs", "new_tab"].contains(&name.as_str()) {
        value["args"] = json!({});
    }
    let args = value
        .get_mut("args")
        .filter(|v| v.is_object())
        .ok_or(ErrorCode::InvalidMessage)?;
    if !["list_tabs", "new_tab"].contains(&name.as_str())
        && !args["browserId"].as_str().is_some_and(browser_id)
    {
        return Err(ErrorCode::InvalidMessage);
    }
    validate_fields(&name, args)?;
    let valid = match name.as_str() {
        "list_tabs" => args.as_object().is_some_and(serde_json::Map::is_empty),
        "new_tab" => {
            args.as_object()
                .is_some_and(|a| a.keys().all(|k| k == "url"))
                && (args.get("url").is_none() || url(args, "url"))
        }
        "navigate" => url(args, "url"),
        "click" | "fill" | "select" | "hover" | "upload" => reference(args, "ref"),
        "wait" => {
            (text(args, "text") != text(args, "url"))
                && args
                    .get("timeoutMs")
                    .is_none_or(|v| v.as_u64().is_some_and(|n| n > 0 && n <= 30_000))
        }
        "type" => args["text"].is_string(),
        "keypress" => text(args, "key"),
        "evaluate" => text(args, "function"),
        "drag" => reference(args, "sourceRef") && reference(args, "targetRef"),
        "scroll" => args["deltaX"].is_number() && args["deltaY"].is_number(),
        "resize" => positive(args, "width") && positive(args, "height"),
        _ => true,
    };
    if !valid {
        return Err(ErrorCode::InvalidMessage);
    }
    if ["fill", "select"].contains(&name.as_str()) && !args["value"].is_string() {
        return Err(ErrorCode::InvalidMessage);
    }
    if name == "upload"
        && !args["filePaths"].as_array().is_some_and(|a| {
            !a.is_empty() && a.iter().all(|v| v.as_str().is_some_and(|s| !s.is_empty()))
        })
    {
        return Err(ErrorCode::InvalidMessage);
    }
    for key in ["ref"] {
        if args.get(key).is_some() && !reference(args, key) {
            return Err(ErrorCode::InvalidMessage);
        }
    }
    command_defaults(&name, args)?;
    Ok(value)
}
fn command_defaults(name: &str, args: &mut Value) -> Result<(), ErrorCode> {
    if name == "click" {
        for (key, default) in [
            ("button", json!("left")),
            ("doubleClick", json!(false)),
            ("modifiers", json!([])),
        ] {
            if args.get(key).is_none() {
                args[key] = default;
            }
        }
        if !["left", "right", "middle"].contains(&args["button"].as_str().unwrap_or_default())
            || !args["doubleClick"].is_boolean()
            || !args["modifiers"].as_array().is_some_and(|a| {
                a.iter().all(|v| {
                    v.as_str()
                        .is_some_and(|s| ["Alt", "Control", "Meta", "Shift"].contains(&s))
                })
            })
        {
            return Err(ErrorCode::InvalidMessage);
        }
    }
    if name == "screenshot" {
        if args.get("fullPage").is_none() {
            args["fullPage"] = json!(false);
        }
        if !args["fullPage"].is_boolean() {
            return Err(ErrorCode::InvalidMessage);
        }
    }
    if name == "logs" {
        if args.get("maxEntries").is_none() {
            args["maxEntries"] = json!(50);
        }
        if !args["maxEntries"]
            .as_u64()
            .is_some_and(|n| n > 0 && n <= 200)
        {
            return Err(ErrorCode::InvalidMessage);
        }
    }
    Ok(())
}
fn validate_fields(name: &str, args: &Value) -> Result<(), ErrorCode> {
    let extra: &[&str] = match name {
        "list_tabs" | "snapshot" | "back" | "forward" | "reload" | "close_tab" => &[],
        "new_tab" | "navigate" => &["url"],
        "click" => &["ref", "button", "doubleClick", "modifiers"],
        "fill" | "select" => &["ref", "value"],
        "wait" => &["text", "url", "timeoutMs"],
        "type" => &["ref", "text"],
        "keypress" => &["ref", "key"],
        "screenshot" => &["fullPage"],
        "upload" => &["ref", "filePaths"],
        "hover" => &["ref"],
        "drag" => &["sourceRef", "targetRef"],
        "logs" => &["maxEntries"],
        "evaluate" => &["function", "ref"],
        "scroll" => &["ref", "deltaX", "deltaY"],
        "resize" => &["width", "height"],
        _ => return Err(ErrorCode::InvalidMessage),
    };
    if args.as_object().is_some_and(|map| {
        map.keys()
            .any(|key| key != "browserId" && !extra.contains(&key.as_str()))
    }) {
        return Err(ErrorCode::InvalidMessage);
    }
    if name == "wait"
        && ["text", "url"]
            .iter()
            .any(|key| args.get(*key).is_some() && !text(args, key))
    {
        return Err(ErrorCode::InvalidMessage);
    }
    Ok(())
}
/// Validate a reply's success/error union and command-specific identity/result fields.
#[must_use]
pub fn response(value: &Value, expected: &str) -> bool {
    if value.get("dialogs").is_some_and(|dialogs| {
        !dialogs
            .as_array()
            .is_some_and(|items| items.iter().all(dialog))
    }) {
        return false;
    }
    if value["ok"] == false {
        return text(&value["error"], "message")
            && value["error"]["code"].as_str().is_some_and(|s| {
                [
                    "browser_disabled",
                    "browser_no_host",
                    "browser_tab_not_found",
                    "browser_tab_closed",
                    "browser_timeout",
                    "screenshot_no_frame",
                    "browser_denied",
                    "browser_unsupported",
                    "browser_stale_ref",
                    "browser_unknown_error",
                ]
                .contains(&s)
            })
            && value["error"]
                .get("retryable")
                .is_none_or(Value::is_boolean);
    }
    if value["ok"] != true {
        return false;
    }
    let result = &value["result"];
    if result["command"] != expected {
        return false;
    }
    if !optional_result(result) {
        return false;
    }
    if expected == "list_tabs" {
        return result["tabs"].as_array().is_some_and(|tabs| {
            tabs.len() <= 4096
                && tabs.iter().all(|tab| {
                    tab["browserId"].as_str().is_some_and(browser_id)
                        && tab["url"].is_string()
                        && tab["title"].is_string()
                        && optional_result(tab)
                })
        });
    }
    if !result["browserId"].as_str().is_some_and(browser_id) {
        return false;
    }
    match expected {
        "new_tab" => text(result, "workspaceId") && text(result, "url"),
        "snapshot" => {
            result["url"].is_string()
                && result["title"].is_string()
                && result["format"] == "aria-yaml"
                && result["snapshot"].is_string()
                && result["truncated"].is_boolean()
                && ["nodeCount", "refCount", "textLength"]
                    .iter()
                    .all(|k| result["stats"][k].as_u64().is_some())
        }
        "click" | "fill" | "hover" => reference(result, "ref"),
        "select" => reference(result, "ref") && result["value"].is_string(),
        "wait" => result["matched"] == "text" || result["matched"] == "url",
        "keypress" => text(result, "key"),
        "navigate" => text(result, "url"),
        "screenshot" => {
            result["mimeType"] == "image/png"
                && text(result, "dataBase64")
                && result["width"].as_u64().is_some()
                && result["height"].as_u64().is_some()
        }
        "upload" => {
            reference(result, "ref")
                && result["filePaths"].as_array().is_some_and(|a| {
                    !a.is_empty() && a.iter().all(|v| v.as_str().is_some_and(|s| !s.is_empty()))
                })
        }
        "drag" => reference(result, "sourceRef") && reference(result, "targetRef"),
        "logs" => logs(result),
        "evaluate" => result["resultJson"].is_string() && result["truncated"].is_boolean(),
        "scroll" => result["deltaX"].is_number() && result["deltaY"].is_number(),
        "resize" => positive(result, "width") && positive(result, "height"),
        _ => true,
    }
}

fn logs(result: &Value) -> bool {
    result["console"].as_array().is_some_and(|entries| {
        entries.iter().all(|entry| {
            entry["level"].is_string()
                && entry["message"].is_string()
                && entry["timestamp"].is_number()
                && entry.get("source").is_none_or(Value::is_string)
                && entry.get("line").is_none_or(|v| v.as_i64().is_some())
        })
    }) && result["network"].as_array().is_some_and(|entries| {
        entries.iter().all(|entry| {
            entry["url"].is_string()
                && entry["startTime"].is_number()
                && entry["duration"].is_number()
                && ["method", "type"]
                    .iter()
                    .all(|key| entry.get(*key).is_none_or(Value::is_string))
                && entry.get("status").is_none_or(|v| v.as_i64().is_some())
                && entry.get("transferSize").is_none_or(Value::is_number)
        })
    })
}
fn optional_result(value: &Value) -> bool {
    value
        .get("workspaceId")
        .is_none_or(|v| v.as_str().is_some_and(|s| !s.is_empty()))
        && ["isActive", "isLoading", "canGoBack", "canGoForward"]
            .iter()
            .all(|key| value.get(*key).is_none_or(Value::is_boolean))
        && ["x", "y", "sourceX", "sourceY", "targetX", "targetY"]
            .iter()
            .all(|key| value.get(*key).is_none_or(Value::is_number))
        && value.get("ref").is_none_or(|_| reference(value, "ref"))
        && value.get("stats").is_none_or(|stats| {
            stats.as_object().is_some_and(|map| {
                map.iter().all(|(key, v)| {
                    [
                        "nodeCount",
                        "refCount",
                        "textLength",
                        "iframeCount",
                        "maxDepth",
                    ]
                    .contains(&key.as_str())
                        && v.as_u64().is_some()
                })
            })
        })
}
fn dialog(value: &Value) -> bool {
    value["type"]
        .as_str()
        .is_some_and(|s| ["alert", "confirm", "prompt", "beforeunload"].contains(&s))
        && value["message"].is_string()
        && value["action"]
            .as_str()
            .is_some_and(|s| ["accepted", "dismissed"].contains(&s))
        && value["timestamp"].is_number()
        && ["defaultValue", "promptText"]
            .iter()
            .all(|key| value.get(*key).is_none_or(Value::is_string))
}
/// Apply upstream reply defaults after validation.
pub(crate) fn defaults(value: &mut Value) {
    if value["ok"] == false && value["error"].get("retryable").is_none() {
        value["error"]["retryable"] = json!(false);
    }
    if let Some(tabs) = value["result"]["tabs"].as_array_mut() {
        for tab in tabs {
            for key in ["isActive", "isLoading"] {
                if tab.get(key).is_none() {
                    tab[key] = json!(false);
                }
            }
        }
    }
}
#[cfg(test)]
mod tests;
