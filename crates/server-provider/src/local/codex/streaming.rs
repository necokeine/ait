use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Value, json};
use uuid::Uuid;

use super::discovery;
use crate::ports::agent_session::{AgentSessionError, AgentTurnEvent};
use crate::protocol::timeline::NativeItem;

const MAX_ITEMS: usize = 4096;
const MAX_TEXT: usize = 192 * 1024;
const MAX_OUTPUT: usize = 16 * 1024;

pub(super) fn steer_rejected(error: &Value) -> bool {
    if error["code"] == -32601 {
        return true;
    }
    if error["code"] != -32600 {
        return false;
    }
    if error
        .pointer("/data/codexErrorInfo/activeTurnNotSteerable")
        .is_some_and(Value::is_object)
    {
        return true;
    }
    let Some(message) = error["message"].as_str() else {
        return false;
    };
    if matches!(
        message,
        "no active turn to steer" | "active turn uses a different output schema"
    ) {
        return true;
    }
    message
        .strip_prefix("expected active turn id `")
        .and_then(|message| message.strip_suffix('`'))
        .and_then(|message| message.split_once("` but found `"))
        .is_some_and(|(expected, actual)| {
            !expected.is_empty()
                && !actual.is_empty()
                && !expected.contains('`')
                && !actual.contains('`')
        })
}

#[derive(Debug, Default)]
pub(super) struct Stream {
    pub(super) events: std::collections::VecDeque<AgentTurnEvent>,
    completed: BTreeSet<String>,
    text_bytes: BTreeMap<String, usize>,
    summary_indices: BTreeMap<String, u64>,
    tools: BTreeMap<String, Value>,
}

impl Stream {
    pub(super) fn complete(&mut self, item: &Value) -> Result<bool, AgentSessionError> {
        let id = text(item, "id")?;
        if self.completed.contains(id) {
            return Ok(false);
        }
        if self.completed.len() >= MAX_ITEMS {
            return Err(AgentSessionError::Failed);
        }
        self.text_bytes.remove(id);
        self.summary_indices.remove(id);
        self.tools.remove(id);
        Ok(self.completed.insert(id.to_owned()))
    }

    pub(super) fn progress(
        &mut self,
        method: &str,
        params: &Value,
    ) -> Result<Option<AgentTurnEvent>, AgentSessionError> {
        let item = match method {
            "item/agentMessage/delta" | "item/reasoning/summaryTextDelta" => {
                self.text_delta(method, params)?
            }
            "item/started" => self.tool_started(&params["item"])?,
            "item/commandExecution/outputDelta" | "item/fileChange/outputDelta" => {
                self.tool_output(params)?
            }
            _ => None,
        };
        let Some((id, item)) = item else {
            return Ok(None);
        };
        let turn = text(params, "turnId")?;
        Ok(Some(AgentTurnEvent::Progress {
            observation: Uuid::new_v4().to_string(),
            entry: NativeItem {
                key: format!("native:{turn}:{id}"),
                turn_id: Some(turn.to_owned()),
                timestamp: discovery::timestamp(),
                item,
            },
        }))
    }

    fn text_delta(
        &mut self,
        method: &str,
        params: &Value,
    ) -> Result<Option<(String, Value)>, AgentSessionError> {
        let id = text(params, "itemId")?;
        let delta = params["delta"].as_str().ok_or(AgentSessionError::Failed)?;
        if self.completed.contains(id) || delta.is_empty() {
            return Ok(None);
        }
        if self.text_bytes.len() >= MAX_ITEMS && !self.text_bytes.contains_key(id) {
            return Err(AgentSessionError::Failed);
        }
        let reasoning = method == "item/reasoning/summaryTextDelta";
        let mut prefix = "";
        if reasoning {
            let index = params["summaryIndex"]
                .as_u64()
                .ok_or(AgentSessionError::Failed)?;
            let previous = self.summary_indices.entry(id.to_owned()).or_insert(0);
            if index < *previous || index > *previous + 1 {
                return Err(AgentSessionError::Failed);
            }
            if index > *previous {
                prefix = "\n";
            }
            *previous = index;
        }
        let bytes = self.text_bytes.entry(id.to_owned()).or_default();
        *bytes = bytes.saturating_add(delta.len() + prefix.len());
        if *bytes > MAX_TEXT {
            return Err(AgentSessionError::Failed);
        }
        let mut item = json!({"type":if reasoning {"reasoning"} else {"assistant_message"},
            "text":format!("{prefix}{delta}")});
        if !reasoning {
            item["messageId"] = json!(id);
        }
        Ok(Some((id.to_owned(), item)))
    }

    fn tool_started(
        &mut self,
        native: &Value,
    ) -> Result<Option<(String, Value)>, AgentSessionError> {
        let kind = text(native, "type")?;
        if !matches!(
            kind,
            "commandExecution"
                | "fileChange"
                | "mcpToolCall"
                | "dynamicToolCall"
                | "webSearch"
                | "collabAgentToolCall"
        ) {
            return Ok(None);
        }
        let id = text(native, "id")?;
        if self.completed.contains(id) || self.tools.contains_key(id) {
            return Ok(None);
        }
        if self.tools.len() >= MAX_ITEMS {
            return Err(AgentSessionError::Failed);
        }
        let item = json!({"type":"tool_call","callId":id,"name":kind,"status":"running",
            "error":null,"detail":crate::local::tool_detail::codex(native)});
        self.tools.insert(id.to_owned(), item.clone());
        Ok(Some((id.to_owned(), item)))
    }

    fn tool_output(
        &mut self,
        params: &Value,
    ) -> Result<Option<(String, Value)>, AgentSessionError> {
        let id = text(params, "itemId")?;
        let delta = params["delta"].as_str().ok_or(AgentSessionError::Failed)?;
        let Some(item) = self.tools.get_mut(id) else {
            return Ok(None);
        };
        if delta.is_empty() {
            return Ok(None);
        }
        let output = item["detail"]["output"].as_str().unwrap_or_default();
        // The live preview is a bounded tail. Complete native output remains in the final item.
        let mut tail = String::with_capacity(MAX_OUTPUT.min(output.len() + delta.len()));
        if delta.len() < MAX_OUTPUT {
            tail.push_str(output);
        }
        let mut start = delta.len().saturating_sub(MAX_OUTPUT);
        while !delta.is_char_boundary(start) {
            start += 1;
        }
        tail.push_str(&delta[start..]);
        let mut start = tail.len().saturating_sub(MAX_OUTPUT);
        while !tail.is_char_boundary(start) {
            start += 1;
        }
        if start > 0 {
            tail.drain(..start);
        }
        item["detail"]["output"] = json!(tail);
        Ok(Some((id.to_owned(), item.clone())))
    }
}

fn text<'a>(value: &'a Value, key: &str) -> Result<&'a str, AgentSessionError> {
    value[key]
        .as_str()
        .filter(|value| !value.is_empty() && value.len() <= 1024)
        .ok_or(AgentSessionError::Failed)
}

#[cfg(test)]
mod tests;
