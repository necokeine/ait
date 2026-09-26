//! Native controls run beside foreground input; they never become an inference prompt.

use serde_json::{Value, json};

use super::{CodexSession, discovery};
use crate::ports::agent_session::{AgentSessionError, AgentTurnEvent};
use crate::protocol::{prompt::AgentPrompt, timeline::NativeItem};

pub(super) fn split(text: &str) -> Option<(&str, &str)> {
    let command = text.trim().strip_prefix('/')?;
    let (name, args) = command
        .split_once(char::is_whitespace)
        .unwrap_or((command, ""));
    (!name.is_empty() && !name.contains('/')).then_some((name, args.trim()))
}

pub(super) fn out_of_band(text: &str) -> bool {
    split(text).is_some_and(|(name, _)| matches!(name, "compact" | "goal"))
}

impl CodexSession {
    pub(super) async fn execute_command(
        &mut self,
        prompt: &AgentPrompt,
    ) -> Result<(), AgentSessionError> {
        prompt.validate()?;
        if self.history
            || !prompt.images.is_empty()
            || !prompt.attachments.is_empty()
            || prompt.output_schema.is_some()
        {
            return Err(AgentSessionError::Rejected);
        }
        let (name, args) = split(&prompt.text).ok_or(AgentSessionError::Rejected)?;
        let (method, params, message) = match name {
            "compact" if args.is_empty() && self.manual_compactions == 0 => (
                Some("thread/compact/start"),
                json!({"threadId":self.id}),
                "Context compaction requested.".to_owned(),
            ),
            "goal" if self.client.goals() => goal(&self.id, args),
            _ => return Err(AgentSessionError::Rejected),
        };
        let message_id = prompt
            .client_message_id
            .as_deref()
            .ok_or(AgentSessionError::Rejected)?;
        let entry = NativeItem {
            key: format!("native:codex:control:{message_id}"),
            turn_id: self
                .active_turn
                .clone()
                .or_else(|| self.last_anchor.clone()),
            timestamp: discovery::timestamp(),
            item: json!({"type":"assistant_message","messageId":format!("control:{message_id}"),"text":message}),
        };
        // Capacity validation precedes any native mutation, so rejection is definitive.
        let mut notes = self.notes.clone();
        notes.push(entry.clone())?;
        let goal_wait = method == Some("thread/goal/set")
            && params["status"] == "active"
            && self.active_turn.is_none();
        if let Some(method) = method {
            self.transport
                .as_mut()
                .ok_or(AgentSessionError::Failed)?
                .request(method, params)
                .await?;
        }
        if name == "compact" {
            self.manual_compactions += 1;
        }
        if name == "goal" && method.is_some() {
            self.pending_goal_start = goal_wait;
        }
        self.notes = notes;
        self.stream
            .events
            .push_back(AgentTurnEvent::Timeline(entry));
        Ok(())
    }

    pub(super) fn control_event(
        &mut self,
        method: &str,
        params: &Value,
    ) -> Result<Option<AgentTurnEvent>, AgentSessionError> {
        if method == "thread/goal/updated" && params["goal"]["status"] != "active"
            || method == "thread/goal/cleared"
        {
            self.pending_goal_start = false;
        }
        if method == "turn/plan/updated" {
            self.plan_progress(params)?;
            return Ok(self.stream.events.pop_front());
        }
        if method == "thread/tokenUsage/updated" {
            return Ok(crate::local::usage::codex(&params["tokenUsage"]).map(AgentTurnEvent::Usage));
        }
        if method == "turn/started" {
            self.pending_goal_start = false;
            let id = params["turn"]["id"]
                .as_str()
                .filter(|id| !id.is_empty() && id.len() <= 512)
                .ok_or(AgentSessionError::Failed)?;
            if self.active_turn.as_deref() == Some(id) {
                return Ok(None);
            }
            if self.active_turn.is_some() {
                return Err(AgentSessionError::Failed);
            }
            self.active_turn = Some(id.to_owned());
            self.last_message = None;
            return Ok(Some(AgentTurnEvent::Started(id.to_owned())));
        }
        if method == "item/completed"
            && params["item"]["type"] == "contextCompaction"
            && self.manual_compactions > 0
        {
            if !self.stream.complete(&params["item"])? {
                return Ok(None);
            }
            self.manual_compactions -= 1;
            let turn = params["turnId"].as_str().ok_or(AgentSessionError::Failed)?;
            return discovery::timeline_item(&params["item"], turn, &discovery::timestamp())
                .map(|entry| entry.map(AgentTurnEvent::Timeline));
        }
        Ok(None)
    }
}

fn goal(thread: &str, args: &str) -> (Option<&'static str>, Value, String) {
    match args.to_ascii_lowercase().as_str() {
        "" => (
            None,
            Value::Null,
            "Usage: /goal <objective>|pause|resume|clear".to_owned(),
        ),
        "pause" => (
            Some("thread/goal/set"),
            json!({"threadId":thread,"status":"paused"}),
            "Goal paused.".to_owned(),
        ),
        "resume" => (
            Some("thread/goal/set"),
            json!({"threadId":thread,"status":"active"}),
            "Goal resumed.".to_owned(),
        ),
        "clear" => (
            Some("thread/goal/clear"),
            json!({"threadId":thread}),
            "Goal cleared.".to_owned(),
        ),
        _ => (
            Some("thread/goal/set"),
            json!({"threadId":thread,"objective":args,"status":"active"}),
            format!("Goal set: {args}"),
        ),
    }
}

#[cfg(test)]
mod tests;

pub(super) async fn input(
    transport: &mut super::Transport,
    cwd: &str,
    prompt: &AgentPrompt,
) -> Result<Vec<Value>, AgentSessionError> {
    let mut input = prompt.codex_input()?;
    let Some((name, args)) = split(&prompt.text) else {
        return Ok(input);
    };
    let command = if let Some(name) = name.strip_prefix("prompts:") {
        let text = super::prompts::invoke(&super::prompts::directory()?, name, args)?;
        vec![json!({"type":"text","text":text,"text_elements":[]})]
    } else {
        let response = transport
            .request("skills/list", json!({"cwds":[cwd],"forceReload":true}))
            .await?;
        let skill = super::controls::skills(&response, cwd)?
            .into_iter()
            .find(|skill| skill.name == name)
            .ok_or(AgentSessionError::Rejected)?;
        let text = if args.is_empty() {
            format!("${name}")
        } else {
            format!("${name} {args}")
        };
        vec![
            json!({"type":"skill","name":skill.name,"path":skill.path}),
            json!({"type":"text","text":text,"text_elements":[]}),
        ]
    };
    let index = prompt
        .attachments
        .iter()
        .filter(|attachment| attachment["contextKind"] == "chat_history")
        .count();
    input.splice(index..=index, command);
    Ok(input)
}
