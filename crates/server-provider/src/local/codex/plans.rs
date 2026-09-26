//! Durable plan review: approving admits exactly one ordinary implementation prompt.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::ports::agent_session::AgentSessionError;
use crate::protocol::{prompt::AgentPrompt, timeline::NativeItem};

impl super::CodexSession {
    pub(super) fn plan_progress(&mut self, params: &Value) -> Result<(), AgentSessionError> {
        let turn = params["turnId"].as_str().ok_or(AgentSessionError::Failed)?;
        if self.active_turn.as_deref() != Some(turn) {
            return Ok(());
        }
        let steps = params["plan"]
            .as_array()
            .filter(|steps| steps.len() <= 1024)
            .ok_or(AgentSessionError::Failed)?;
        let mut items = Vec::new();
        for (index, step) in steps.iter().enumerate() {
            let Some(text) = step["step"]
                .as_str()
                .map(str::trim)
                .filter(|text| !text.is_empty())
            else {
                continue;
            };
            if text.len() > 8192 {
                return Err(AgentSessionError::Failed);
            }
            let status = match step["status"].as_str() {
                Some("completed") => "completed",
                Some("inProgress" | "in_progress") => "in_progress",
                _ => "pending",
            };
            items.push(json!({"id":index.to_string(),"text":text,"status":status,"completed":status=="completed"}));
        }
        let plan_mode = self
            .config
            .feature_values
            .as_ref()
            .is_some_and(|features| features.get("plan_mode") == Some(&json!(true)));
        if plan_mode {
            let text = items
                .iter()
                .filter_map(|item| item["text"].as_str())
                .map(|text| format!("- {text}"))
                .collect::<Vec<_>>()
                .join("\n");
            self.latest_plan =
                Plan::receive(&json!({"id":format!("plan:{turn}"),"text":text}), turn)?;
        } else {
            let item = json!({"type":"todo","items":items});
            let hash =
                Sha256::digest(serde_json::to_vec(&item).map_err(|_| AgentSessionError::Failed)?);
            let entry = NativeItem {
                key: format!("native:codex:plan-progress:{turn}:{hash:x}"),
                turn_id: Some(turn.to_owned()),
                timestamp: super::discovery::timestamp(),
                item,
            };
            self.notes.push(entry.clone())?;
            self.stream
                .events
                .push_back(crate::ports::agent_session::AgentTurnEvent::Timeline(entry));
        }
        Ok(())
    }

    pub(super) fn plan_patch(
        &self,
        id: &str,
        response: &Value,
    ) -> Result<Option<crate::protocol::agent_config::ConfigPatch>, AgentSessionError> {
        let Some(plan) = self
            .pending_plan
            .as_ref()
            .filter(|plan| plan.request_id() == id)
        else {
            return Ok(None);
        };
        if plan.prepare(response)?.is_none() {
            return Ok(None);
        }
        Ok(Some(crate::protocol::agent_config::ConfigPatch {
            feature_values: Some(std::collections::BTreeMap::from([(
                "plan_mode".to_owned(),
                json!(false),
            )])),
            ..crate::protocol::agent_config::ConfigPatch::default()
        }))
    }

    pub(super) fn resolve_plan(&mut self, response: &Value) -> Result<(), AgentSessionError> {
        let plan = self
            .pending_plan
            .as_ref()
            .ok_or(AgentSessionError::Rejected)?;
        let entry = plan.resolution(response)?;
        self.notes.push(entry.clone())?;
        self.stream
            .events
            .push_back(crate::ports::agent_session::AgentTurnEvent::Timeline(entry));
        self.pending_plan = None;
        Ok(())
    }

    pub(super) fn preflight_plan(&self, prompt: &AgentPrompt) -> Result<(), AgentSessionError> {
        if let Some(plan) = self
            .pending_plan
            .as_ref()
            .filter(|plan| prompt.client_message_id.as_deref() != Some(&plan.prompt_id()))
        {
            let mut notes = self.notes.clone();
            notes.push(plan.resolution(&json!({"behavior":"deny"}))?)?;
        }
        Ok(())
    }

    pub(super) fn dismiss_plan_for(
        &mut self,
        prompt: &AgentPrompt,
    ) -> Result<(), AgentSessionError> {
        if let Some(plan) = self
            .pending_plan
            .as_ref()
            .filter(|plan| prompt.client_message_id.as_deref() != Some(&plan.prompt_id()))
        {
            let id = plan.request_id();
            self.resolve_plan(&json!({"behavior":"deny"}))?;
            self.stream
                .events
                .push_back(crate::ports::agent_session::AgentTurnEvent::PermissionResolved(id));
        }
        Ok(())
    }

    pub(super) fn finish_plan(&mut self, completed: bool) -> Result<(), AgentSessionError> {
        let latest = self.latest_plan.take();
        if completed && let Some(plan) = latest {
            if let Some(previous) = &self.pending_plan {
                let id = previous.request_id();
                self.resolve_plan(&json!({"behavior":"deny"}))?;
                self.stream
                    .events
                    .push_back(crate::ports::agent_session::AgentTurnEvent::PermissionResolved(id));
            }
            self.stream.events.push_back(
                crate::ports::agent_session::AgentTurnEvent::PermissionRequested(plan.request()),
            );
            self.pending_plan = Some(plan);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct Plan {
    id: String,
    turn: String,
    text: String,
    timestamp: String,
}

impl Plan {
    pub(super) fn receive(item: &Value, turn: &str) -> Result<Option<Self>, AgentSessionError> {
        let text = item["text"].as_str().unwrap_or_default().trim();
        if text.is_empty() {
            return Ok(None);
        }
        let plan = Self {
            id: item["id"]
                .as_str()
                .ok_or(AgentSessionError::Failed)?
                .to_owned(),
            turn: turn.to_owned(),
            text: text.to_owned(),
            timestamp: super::discovery::timestamp(),
        };
        Self::restore(Some(&json!(plan)))
    }

    pub(super) fn restore(saved: Option<&Value>) -> Result<Option<Self>, AgentSessionError> {
        let Some(saved) = saved else { return Ok(None) };
        let plan: Self =
            serde_json::from_value(saved.clone()).map_err(|_| AgentSessionError::Failed)?;
        if plan.id.is_empty()
            || plan.id.len() > 256
            || plan.turn.is_empty()
            || plan.turn.len() > 256
            || plan.text.trim().is_empty()
            || plan.text.len() > 48 * 1024
            || chrono::DateTime::parse_from_rfc3339(&plan.timestamp).is_err()
        {
            return Err(AgentSessionError::Failed);
        }
        Ok(Some(plan))
    }

    pub(super) fn request_id(&self) -> String {
        format!(
            "plan-approval:{:x}",
            Sha256::digest(format!("{}:{}", self.turn, self.id))
        )
    }

    pub(super) fn prompt_id(&self) -> String {
        format!("implement:{}", self.request_id())
    }

    pub(super) fn request(&self) -> Value {
        json!({"id":self.request_id(),"provider":"codex","name":"CodexPlanApproval","kind":"plan",
            "title":"Plan","description":"Review the proposed plan before implementation starts.",
            "input":{"plan":self.text},"metadata":{"planText":self.text,"source":"codex_plan_approval"},
            "actions":[{"id":"dismiss","label":"Dismiss","behavior":"deny","variant":"danger","intent":"dismiss"},
                {"id":"implement","label":"Implement","behavior":"allow","variant":"primary","intent":"implement"}]})
    }

    pub(super) fn prepare(
        &self,
        response: &Value,
    ) -> Result<Option<AgentPrompt>, AgentSessionError> {
        if !allowed(response)? {
            return Ok(None);
        }
        let prompt = AgentPrompt {
            text: format!(
                "The user approved the plan. Implement it now. Do not restate or revise the plan unless blocked.\n\nApproved plan:\n\n{}\n\nCarry out the work, make the necessary code changes, and verify the result.",
                self.text
            ),
            client_message_id: Some(self.prompt_id()),
            ..AgentPrompt::default()
        };
        prompt.validate()?;
        Ok(Some(prompt))
    }

    pub(super) fn resolution(&self, response: &Value) -> Result<NativeItem, AgentSessionError> {
        let approved = allowed(response)?;
        Ok(NativeItem {
            key: format!("native:{}:resolution", self.request_id()),
            turn_id: Some(self.turn.clone()),
            timestamp: self.timestamp.clone(),
            item: json!({"type":"tool_call","callId":self.request_id(),
                "name":"plan_approval","status":"completed","error":null,
                "detail":{"type":"plan","text":self.text},"metadata":{"resolution":if approved {"approved"} else {"dismissed"}}}),
        })
    }

    pub(super) fn retained(&self, entries: &[NativeItem]) -> bool {
        let key = format!("native:{}:{}", self.turn, self.id);
        entries.iter().any(|entry| entry.key == key)
    }
}

fn allowed(response: &Value) -> Result<bool, AgentSessionError> {
    if response["scope"]
        .as_str()
        .is_some_and(|scope| scope != "once")
        || response.get("updatedPermissions").is_some()
        || response.get("updatedInput").is_some_and(|input| {
            !input.is_null() && input.as_object().is_none_or(|input| !input.is_empty())
        })
    {
        return Err(AgentSessionError::Rejected);
    }
    match response["behavior"].as_str() {
        Some("allow") => Ok(true),
        Some("deny") => Ok(false),
        _ => Err(AgentSessionError::Rejected),
    }
}

#[cfg(test)]
mod tests;
