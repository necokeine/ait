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
    append(&mut text, "\n")?;
    append_body(&mut text, selected)?;
    append(&mut text, "</chat-history-summary>")?;
    super::timeline::bounded(json!({"agentId":request.agent_id,"attachment":{
        "type":"text","mimeType":"text/plain","contextKind":"chat_history","title":"Chat history","text":text},
        "itemCount":selected.len(),"boundaryCursor":request.boundary_cursor,"boundaryMessageId":message,"error":null}))
}

fn append_body(text: &mut String, selected: &[Row]) -> Result<(), ErrorCode> {
    let mut rows = selected.iter().peekable();
    let mut message = String::new();
    let mut body = false;
    while let Some(row) = rows.next() {
        let item = &row.entry.item;
        match item["type"].as_str() {
            Some("user_message") => {
                body |= append_message(text, "[User] ", item["text"].as_str().unwrap_or(""))?;
            }
            Some("assistant_message") => {
                message.clear();
                append(&mut message, item["text"].as_str().unwrap_or(""))?;
                let mut previous = row;
                // Only adjacent source fragments can be joined. A user, tool, different
                // message or missing sequence must remain between the surrounding text.
                while let Some(next) = rows.next_if(|next| {
                    next.entry.item["type"] == "assistant_message"
                        && next.entry.key == row.entry.key
                        && next.entry.turn_id == row.entry.turn_id
                        && previous.seq.checked_add(1) == Some(next.seq)
                }) {
                    append(&mut message, next.entry.item["text"].as_str().unwrap_or(""))?;
                    previous = next;
                }
                body |= append_message(text, "[Assistant] ", &message)?;
            }
            Some("tool_call") => {
                // Raw tool inputs, plugin payloads and reasoning are not context attachment text.
                let name = item["name"].as_str().unwrap_or("Tool");
                append(text, &format!("[{}]\n", name.trim()))?;
                body = true;
            }
            _ => {}
        }
    }
    if !body {
        append(text, "No chat history to display.\n")?;
    }
    Ok(())
}

fn append_message(output: &mut String, label: &str, text: &str) -> Result<bool, ErrorCode> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(false);
    }
    append(output, label)?;
    append(output, text)?;
    append(output, "\n")?;
    Ok(true)
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
