//! Native tool display projections. Execution and the complete raw output stay provider-owned.

use serde_json::{Value, json};
use std::borrow::Cow;

const PREVIEW_BYTES: usize = 128 * 1024;

pub(super) fn claude(name: &str, input: &Value) -> Value {
    let mut detail = match name {
        "Bash" if input["command"].is_string() => {
            json!({"type":"shell","command":input["command"]})
        }
        "Read" if input["file_path"].is_string() => {
            json!({"type":"read","filePath":input["file_path"]})
        }
        "Edit" if input["file_path"].is_string() => {
            json!({"type":"edit","filePath":input["file_path"]})
        }
        "Write" if input["file_path"].is_string() => {
            json!({"type":"write","filePath":input["file_path"]})
        }
        "Glob" | "Grep" => {
            json!({"type":"search","toolName":if name=="Glob" {"glob"} else {"grep"},"query":input["pattern"].as_str().unwrap_or("")})
        }
        "WebSearch" => {
            json!({"type":"search","toolName":"web_search","query":input["query"].as_str().unwrap_or("")})
        }
        "WebFetch" if input["url"].is_string() => json!({"type":"fetch","url":input["url"]}),
        "Agent" | "Task" | "Workflow" => json!({"type":"sub_agent","log":"","actions":[]}),
        "ExitPlanMode" if input["plan"].is_string() => json!({"type":"plan","text":input["plan"]}),
        _ => return json!({"type":"unknown","input":input,"output":null}),
    };
    for (source, target) in [
        ("cwd", "cwd"),
        ("offset", "offset"),
        ("limit", "limit"),
        ("old_string", "oldString"),
        ("new_string", "newString"),
        ("content", "content"),
        ("prompt", "prompt"),
        ("description", "description"),
        ("subagent_type", "subAgentType"),
    ] {
        if let Some(value) = input.get(source) {
            detail[target] = value.clone();
        }
    }
    detail
}

pub(super) fn claude_result(detail: &mut Value, result: &Value) {
    let output = output_text(result);
    let field = match detail["type"].as_str() {
        Some("shell") => "output",
        Some("read" | "search") => "content",
        Some("fetch") => "result",
        Some("sub_agent") => "log",
        Some("unknown") => {
            detail["output"] = if result.is_string() {
                json!(output)
            } else {
                result.clone()
            };
            return;
        }
        _ => return,
    };
    detail[field] = json!(output);
}

pub(super) fn codex(native: &Value) -> Value {
    match native["type"].as_str() {
        Some("commandExecution") => {
            let mut detail = json!({"type":"shell","command":native["command"].as_str().unwrap_or(""),"output":""});
            if let Some(cwd) = native["cwd"].as_str() {
                detail["cwd"] = json!(cwd);
            }
            if let Some(output) = native["aggregatedOutput"].as_str() {
                detail["output"] = json!(preview(output));
            }
            if let Some(exit) = native["exitCode"].as_i64() {
                detail["exitCode"] = json!(exit);
            }
            detail
        }
        Some("fileChange") => file_change(native).unwrap_or_else(|| unknown(native)),
        Some("webSearch") => {
            if let Some(url) = native["action"]["url"].as_str() {
                json!({"type":"fetch","url":url})
            } else {
                json!({"type":"search","toolName":"web_search","query":native["query"].as_str().or_else(||native["action"]["query"].as_str()).unwrap_or("")})
            }
        }
        Some("collabAgentToolCall") => {
            let mut detail = json!({"type":"sub_agent","description":native["prompt"].as_str().unwrap_or(""),
                "log":native["tool"].as_str().unwrap_or(""),"actions":[]});
            if let Some(id) = native["receiverThreadIds"]
                .as_array()
                .and_then(|ids| ids.first())
                .and_then(Value::as_str)
            {
                detail["childSessionId"] = json!(id);
            }
            detail
        }
        Some("imageView" | "imageGeneration") => {
            json!({"type":"plain_text","label":"Image","text":native["path"].as_str().unwrap_or("")})
        }
        _ => unknown(native),
    }
}

fn file_change(native: &Value) -> Option<Value> {
    let changes = native["changes"].as_array()?;
    let first = changes.first()?;
    let path = if changes.len() == 1 {
        first["path"].as_str()?
    } else {
        "Multiple files"
    };
    let mut diff = String::new();
    for change in changes.iter().take(256) {
        if diff.len() >= PREVIEW_BYTES {
            break;
        }
        if let Some(text) = change["diff"].as_str() {
            if !diff.is_empty() {
                diff.push('\n');
            }
            diff.push_str(&preview(text));
        }
    }
    Some(json!({"type":"edit","filePath":path,"unifiedDiff":preview(&diff)}))
}

fn unknown(native: &Value) -> Value {
    json!({"type":"unknown","input":native,"output":null})
}

fn output_text(value: &Value) -> String {
    if let Some(text) = value.as_str() {
        return preview(text).into_owned();
    }
    if let Some(blocks) = value.as_array() {
        let mut output = String::new();
        for block in blocks {
            if output.len() >= PREVIEW_BYTES {
                break;
            }
            if let Some(text) = block["text"].as_str() {
                if !output.is_empty() {
                    output.push('\n');
                }
                output.push_str(&preview(text));
            }
        }
        return preview(&output).into_owned();
    }
    preview(&value.to_string()).into_owned()
}

fn preview(text: &str) -> Cow<'_, str> {
    const MARKER: &str = "\n[Output truncated; full output remains in the native transcript.]";
    if text.len() <= PREVIEW_BYTES {
        return Cow::Borrowed(text);
    }
    let mut end = PREVIEW_BYTES - MARKER.len();
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    Cow::Owned(format!("{}{MARKER}", &text[..end]))
}

#[cfg(test)]
mod tests;
