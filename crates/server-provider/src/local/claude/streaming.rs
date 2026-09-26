use std::collections::{BTreeMap, BTreeSet, VecDeque};

use chrono::Utc;
use serde_json::{Value, json};
use uuid::Uuid;

use super::config::text;
use crate::ports::agent_session::{AgentSessionError, AgentTurnEvent};
use crate::protocol::timeline::NativeItem;

const MAX_ITEMS: usize = 4096;
const MAX_TEXT: usize = 192 * 1024;

#[derive(Debug, Default)]
pub(super) struct Stream {
    images: crate::local::images::ImageStore,
    pub(super) events: VecDeque<AgentTurnEvent>,
    pub(super) last_message: Option<String>,
    pub(super) tools: BTreeMap<String, Value>,
    pub(super) tasks: super::tasks::Tasks,
    completed: BTreeSet<String>,
    messages: BTreeMap<String, usize>,
    partial_bytes: BTreeMap<String, usize>,
    message_id: Option<String>,
    blocks: BTreeMap<u64, String>,
}

impl Stream {
    pub(super) fn observed_assistant(&self, record: &Value) -> bool {
        record["uuid"]
            .as_str()
            .is_some_and(|id| self.completed.contains(&format!("record:{id}")))
    }

    pub(super) fn new(images: crate::local::images::ImageStore) -> Self {
        Self {
            images,
            ..Self::default()
        }
    }

    pub(super) fn result_text(
        &mut self,
        record: &Value,
        turn: &str,
    ) -> Result<Option<String>, AgentSessionError> {
        let structured = record
            .get("structured_output")
            .filter(|output| !output.is_null());
        let text = if let Some(output) = structured {
            Some(format!(
                "```json\n{}\n```",
                serde_json::to_string_pretty(output).map_err(|_| AgentSessionError::Failed)?
            ))
        } else {
            record["result"]
                .as_str()
                .filter(|text| !text.is_empty())
                .map(str::to_owned)
        };
        if let Some(text) = &text
            && (structured.is_some() || self.last_message.is_none())
        {
            let id = format!("result:{}", record["uuid"].as_str().unwrap_or(turn));
            self.complete(
                &id,
                json!({"type":"assistant_message","messageId":id,"text":text}),
                record,
            )?;
        }
        Ok(text.or_else(|| self.last_message.take()))
    }

    pub(super) fn record(&mut self, record: &Value) -> Result<(), AgentSessionError> {
        // Child transcripts belong to the provider, not to the parent's assistant output.
        if record
            .get("parent_tool_use_id")
            .is_some_and(|id| !id.is_null())
            || record["isSidechain"] == true
        {
            return Ok(());
        }
        if self.events.len() >= MAX_ITEMS
            || self.completed.len() >= MAX_ITEMS
            || self.tools.len() >= MAX_ITEMS
        {
            return Err(AgentSessionError::Failed);
        }
        for (id, snapshot) in self.tasks.observe(record)? {
            self.complete(&id, snapshot, record)?;
        }
        match record["type"].as_str() {
            Some("stream_event") => self.partial(&record["event"]),
            Some("assistant") => self.assistant(record),
            Some("user") => self.user(record),
            Some("system") if record["subtype"] == "compact_boundary" => {
                let id = text(record, "uuid")?;
                self.complete(
                    id,
                    json!({"type":"compaction","status":"completed"}),
                    record,
                )
            }
            _ => Ok(()),
        }
    }

    fn partial(&mut self, event: &Value) -> Result<(), AgentSessionError> {
        match event["type"].as_str() {
            Some("message_start") => {
                self.message_id = Some(text(&event["message"], "id")?.to_owned());
                self.blocks.clear();
            }
            Some("content_block_start") => {
                if self.blocks.len() >= MAX_ITEMS {
                    return Err(AgentSessionError::Failed);
                }
                let index = event["index"].as_u64().ok_or(AgentSessionError::Failed)?;
                self.blocks
                    .insert(index, text(&event["content_block"], "type")?.to_owned());
            }
            Some("content_block_delta") => {
                let index = event["index"].as_u64().ok_or(AgentSessionError::Failed)?;
                let kind = self.blocks.get(&index).ok_or(AgentSessionError::Failed)?;
                let field = match (kind.as_str(), event["delta"]["type"].as_str()) {
                    ("text", Some("text_delta")) => "text",
                    ("thinking", Some("thinking_delta")) => "thinking",
                    _ => return Ok(()),
                };
                let delta = event["delta"][field]
                    .as_str()
                    .ok_or(AgentSessionError::Failed)?;
                if delta.is_empty() {
                    return Ok(());
                }
                let message = self.message_id.as_ref().ok_or(AgentSessionError::Failed)?;
                let id = format!("{message}:{index}");
                if self.completed.contains(&id) {
                    return Ok(());
                }
                if self.partial_bytes.len() >= MAX_ITEMS && !self.partial_bytes.contains_key(&id) {
                    return Err(AgentSessionError::Failed);
                }
                let bytes = self.partial_bytes.entry(id.clone()).or_default();
                *bytes = bytes.saturating_add(delta.len());
                if *bytes > MAX_TEXT {
                    return Err(AgentSessionError::Failed);
                }
                let mut item = json!({"type":if field=="text" {"assistant_message"} else {"reasoning"},"text":delta});
                if field == "text" {
                    item["messageId"] = json!(id);
                }
                self.events.push_back(AgentTurnEvent::Progress {
                    observation: Uuid::new_v4().to_string(),
                    entry: entry(&id, item, &Value::Null),
                });
            }
            _ => {}
        }
        Ok(())
    }

    fn assistant(&mut self, record: &Value) -> Result<(), AgentSessionError> {
        let uuid = text(record, "uuid")?;
        if !self.completed.insert(format!("record:{uuid}")) {
            return Ok(());
        }
        let message = &record["message"];
        let id = text(message, "id")?;
        let blocks = message["content"]
            .as_array()
            .filter(|blocks| blocks.len() <= MAX_ITEMS)
            .ok_or(AgentSessionError::Failed)?;
        for block in blocks {
            let index = self.messages.entry(id.to_owned()).or_default();
            let key = format!("{id}:{index}");
            *index += 1;
            match block["type"].as_str() {
                Some("text" | "thinking") => {
                    let thinking = block["type"] == "thinking";
                    let content = block[if thinking { "thinking" } else { "text" }]
                        .as_str()
                        .ok_or(AgentSessionError::Failed)?;
                    if content.len() > MAX_TEXT {
                        return Err(AgentSessionError::Failed);
                    }
                    let mut item = json!({"type":if thinking {"reasoning"} else {"assistant_message"},"text":content});
                    if !thinking {
                        item["messageId"] = json!(key);
                        self.last_message = Some(content.to_owned());
                    }
                    self.complete(&key, item, record)?;
                }
                Some("tool_use") => {
                    let call = text(block, "id")?;
                    let item = json!({"type":"tool_call","callId":call,"name":text(block,"name")?,
                        "status":"running","error":null,"detail":crate::local::tool_detail::claude(text(block,"name")?, &block["input"])});
                    if self.tools.insert(call.to_owned(), item.clone()).is_some() {
                        return Err(AgentSessionError::Failed);
                    }
                    self.events.push_back(AgentTurnEvent::Progress {
                        observation: Uuid::new_v4().to_string(),
                        entry: entry(&format!("tool:{call}"), item, record),
                    });
                }
                Some("image") => {
                    let image = self.images.render(block)?;
                    self.complete_image(&key, &image, record)?;
                }
                // Signatures and redacted thinking are not user-visible content.
                _ => {}
            }
        }
        Ok(())
    }

    fn user(&mut self, record: &Value) -> Result<(), AgentSessionError> {
        if record["isMeta"] == true {
            return Ok(());
        }
        let content = &record["message"]["content"];
        let mut messages = Vec::new();
        if let Some(content) = content.as_str() {
            messages.push(content);
        }
        if let Some(blocks) = content.as_array() {
            for block in blocks {
                if block["type"] == "text" {
                    messages.push(block["text"].as_str().ok_or(AgentSessionError::Failed)?);
                } else if block["type"] == "image" {
                    messages.push("[Image attachment]");
                } else if block["type"] == "tool_result" {
                    let call = text(block, "tool_use_id")?;
                    let key = format!("tool:{call}");
                    if self.completed.contains(&key) {
                        continue;
                    }
                    let mut item = self.tools.remove(call).ok_or(AgentSessionError::Failed)?;
                    let failed = block["is_error"] == true;
                    item["status"] = json!(if failed { "failed" } else { "completed" });
                    item["error"] = json!(failed.then_some("Native tool failed"));
                    let (content, images) = self.images.split(&block["content"])?;
                    crate::local::tool_detail::claude_result(&mut item["detail"], &content);
                    self.complete(&key, item, record)?;
                    for (index, image) in images.iter().enumerate() {
                        self.complete_image(&format!("{key}:image:{index}"), image, record)?;
                    }
                }
            }
        }
        if !messages.is_empty() {
            let id = text(record, "uuid")?;
            self.complete(
                id,
                json!({"type":"user_message","messageId":id,"text":messages.join("\n")}),
                record,
            )?;
        }
        Ok(())
    }

    fn complete_image(
        &mut self,
        id: &str,
        image: &str,
        record: &Value,
    ) -> Result<(), AgentSessionError> {
        self.complete(
            id,
            json!({"type":"assistant_message","messageId":id,"text":image}),
            record,
        )
    }

    fn complete(&mut self, id: &str, item: Value, record: &Value) -> Result<(), AgentSessionError> {
        if serde_json::to_vec(&item)
            .map_err(|_| AgentSessionError::Failed)?
            .len()
            > MAX_TEXT
        {
            return Err(AgentSessionError::Failed);
        }
        if self.completed.insert(id.to_owned()) {
            self.partial_bytes.remove(id);
            self.events
                .push_back(AgentTurnEvent::Timeline(entry(id, item, record)));
        }
        Ok(())
    }
}

pub(super) fn entry(id: &str, item: Value, record: &Value) -> NativeItem {
    NativeItem {
        key: format!("native:claude:{id}"),
        turn_id: None,
        timestamp: record["timestamp"]
            .as_str()
            .map_or_else(|| Utc::now().to_rfc3339(), str::to_owned),
        item,
    }
}
