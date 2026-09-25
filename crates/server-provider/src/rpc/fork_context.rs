//! Text attachment curation from a validated inclusive display-history boundary.

use serde_json::{Value, json};
use server_model::ErrorCode;

use crate::protocol::native_sessions::ForkRequest;
use crate::storage::timeline::Row;

pub(crate) fn export(
    request: &ForkRequest,
    epoch: &str,
    rows: &[Row],
    agent: &Value,
) -> Result<Value, ErrorCode> {
    let message = request
        .boundary_message_id
        .as_deref()
        .map(str::trim)
        .filter(|id| !id.is_empty());
    let end = if let Some(cursor) = &request.boundary_cursor {
        if cursor.epoch != epoch {
            return Err(ErrorCode::InvalidMessage);
        }
        rows.iter()
            .position(|row| row.seq == cursor.seq)
            .ok_or(ErrorCode::InvalidMessage)?
            + 1
    } else if let Some(message) = message {
        rows.iter()
            .rposition(|row| {
                row.entry.item["type"] == "assistant_message"
                    && row.entry.item["messageId"] == message
            })
            .ok_or(ErrorCode::InvalidMessage)?
            + 1
    } else {
        rows.len()
    };
    let selected = &rows[..end];
    let mut text =
        String::from("<chat-history-summary>\nChat history from a previous Paseo agent.\n");
    for (field, label) in [("title", "Source agent"), ("cwd", "Source directory")] {
        if let Some(value) = agent[field]
            .as_str()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            append(&mut text, &format!("{label}: {value}\n"))?;
        }
    }
    text.push('\n');
    let mut body = false;
    let mut assistant = false;
    for row in selected {
        let item = &row.entry.item;
        match item["type"].as_str() {
            Some("user_message" | "assistant_message") => {
                let value = item["text"].as_str().unwrap_or("").trim();
                if value.is_empty() {
                    continue;
                }
                let is_assistant = item["type"] == "assistant_message";
                let label = if is_assistant {
                    if assistant { "" } else { "[Assistant] " }
                } else {
                    "[User] "
                };
                append(&mut text, &format!("{label}{value}\n"))?;
                assistant = is_assistant;
                body = true;
            }
            Some("tool_call") => {
                // Raw tool inputs, plugin payloads and reasoning are not context attachment text.
                let name = item["name"].as_str().unwrap_or("Tool");
                append(&mut text, &format!("[{}]\n", name.trim()))?;
                assistant = false;
                body = true;
            }
            _ => {}
        }
    }
    if !body {
        append(&mut text, "No chat history to display.\n")?;
    }
    text.push_str("</chat-history-summary>");
    super::timeline::bounded(json!({"agentId":request.agent_id,"attachment":{
        "type":"text","mimeType":"text/plain","contextKind":"chat_history","title":"Chat history","text":text},
        "itemCount":selected.len(),"boundaryCursor":request.boundary_cursor,"boundaryMessageId":message,"error":null}))
}

fn append(output: &mut String, text: &str) -> Result<(), ErrorCode> {
    if output.len().saturating_add(text.len()) > 512 * 1024 {
        return Err(ErrorCode::ResourceExhausted);
    }
    output.push_str(text);
    Ok(())
}

#[cfg(test)]
mod tests;
