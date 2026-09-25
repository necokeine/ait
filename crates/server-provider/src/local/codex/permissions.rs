use std::collections::BTreeSet;

use serde_json::{Value, json};
use uuid::Uuid;

use super::CodexSession;
use crate::ports::agent_session::{AgentSessionError, AgentTurnEvent};

#[derive(Debug)]
pub(super) struct Pending {
    native_id: Value,
    kind: Kind,
    pub(super) request: Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Command,
    File,
    Question,
}

pub(super) fn supported(method: &str) -> bool {
    matches!(
        method,
        "item/commandExecution/requestApproval"
            | "item/fileChange/requestApproval"
            | "item/tool/requestUserInput"
    )
}

impl CodexSession {
    pub(super) fn capture_permission(
        &mut self,
        message: &Value,
    ) -> Result<AgentTurnEvent, AgentSessionError> {
        let params = &message["params"];
        if self.active_turn.is_none()
            || params["threadId"] != self.id
            || params["turnId"].as_str() != self.active_turn.as_deref()
            || self.permissions.len() >= 32
            || self
                .permissions
                .values()
                .any(|pending| pending.native_id == message["id"])
            || !(message["id"].is_string() || message["id"].as_i64().is_some())
            || !params["itemId"].is_string()
            || serde_json::to_vec(params)
                .map_err(|_| AgentSessionError::Failed)?
                .len()
                > 65536
        {
            return Err(AgentSessionError::Failed);
        }
        let (kind, name, label) = match message["method"].as_str() {
            Some("item/commandExecution/requestApproval") => {
                (Kind::Command, "commandExecution", "tool")
            }
            Some("item/fileChange/requestApproval") => (Kind::File, "fileChange", "tool"),
            Some("item/tool/requestUserInput") if params["isBlocking"] != false => {
                (Kind::Question, "request_user_input", "question")
            }
            _ => return Err(AgentSessionError::Unavailable),
        };
        if kind == Kind::Question {
            question_ids(&params["questions"])?;
        }
        let id = Uuid::new_v4().to_string();
        let request = json!({"id":id,"provider":"codex","name":name,"kind":label,"input":params,
            "actions":[{"id":"allow","label":"Allow once","behavior":"allow","variant":"primary"},
                {"id":"deny","label":"Deny","behavior":"deny","variant":"secondary"}]});
        self.permissions.insert(
            id,
            Pending {
                native_id: message["id"].clone(),
                kind,
                request: request.clone(),
            },
        );
        Ok(AgentTurnEvent::PermissionRequested(request))
    }

    pub(super) async fn answer_permission(
        &mut self,
        id: &str,
        response: &Value,
    ) -> Result<(), AgentSessionError> {
        let pending = self
            .permissions
            .get(id)
            .ok_or(AgentSessionError::Rejected)?;
        let reply = resolution(pending, response)?;
        let native_id = pending.native_id.clone();
        self.transport
            .as_mut()
            .ok_or(AgentSessionError::Failed)?
            .respond(&native_id, reply)
            .await?;
        self.permissions.remove(id);
        if response["behavior"] == "deny"
            && response["interrupt"] == true
            && let Some(turn) = &self.active_turn
        {
            match self
                .transport
                .as_mut()
                .ok_or(AgentSessionError::Failed)?
                .request("turn/interrupt", json!({"threadId":self.id,"turnId":turn}))
                .await
            {
                Ok(_) | Err(AgentSessionError::Rejected) => {}
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }
}

fn resolution(pending: &Pending, response: &Value) -> Result<Value, AgentSessionError> {
    let object = response.as_object().ok_or(AgentSessionError::Rejected)?;
    let allow = match response["behavior"].as_str() {
        Some("allow") => true,
        Some("deny") => false,
        _ => return Err(AgentSessionError::Rejected),
    };
    let allowed = if allow {
        &["behavior", "selectedActionId", "updatedInput"][..]
    } else {
        &["behavior", "selectedActionId", "message", "interrupt"][..]
    };
    if object.keys().any(|key| !allowed.contains(&key.as_str()))
        || response
            .get("selectedActionId")
            .is_some_and(|action| action != if allow { "allow" } else { "deny" })
        || response
            .get("interrupt")
            .is_some_and(|value| !value.is_boolean())
        || response
            .get("message")
            .is_some_and(|value| !value.is_string())
    {
        return Err(AgentSessionError::Rejected);
    }
    if pending.kind == Kind::Question {
        if !allow {
            return Ok(json!({"answers":{}}));
        }
        if response["updatedInput"]
            .as_object()
            .is_some_and(|input| input.keys().any(|key| key != "answers"))
        {
            return Err(AgentSessionError::Rejected);
        }
        let supplied = response["updatedInput"]["answers"]
            .as_object()
            .ok_or(AgentSessionError::Rejected)?;
        let expected = question_ids(&pending.request["input"]["questions"])?;
        if supplied.len() != expected.len()
            || supplied.keys().any(|id| !expected.contains(id.as_str()))
        {
            return Err(AgentSessionError::Rejected);
        }
        let mut answers = serde_json::Map::new();
        for (id, value) in supplied {
            let values = if let Some(text) = value.as_str() {
                vec![json!(text)]
            } else {
                value
                    .as_array()
                    .cloned()
                    .ok_or(AgentSessionError::Rejected)?
            };
            if values.is_empty()
                || values.len() > 32
                || values
                    .iter()
                    .any(|value| value.as_str().is_none_or(|text| text.len() > 4096))
            {
                return Err(AgentSessionError::Rejected);
            }
            answers.insert(id.clone(), json!({"answers":values}));
        }
        return Ok(json!({"answers":answers}));
    }
    if response.get("updatedInput").is_some() {
        return Err(AgentSessionError::Rejected);
    }
    // Do not turn a one-call decision into a session-wide filesystem permission grant.
    if allow && pending.kind == Kind::File && !pending.request["input"]["grantRoot"].is_null() {
        return Err(AgentSessionError::Rejected);
    }
    Ok(json!({"decision":if allow {"accept"} else {"decline"}}))
}

fn question_ids(questions: &Value) -> Result<BTreeSet<&str>, AgentSessionError> {
    let questions = questions
        .as_array()
        .filter(|questions| !questions.is_empty() && questions.len() <= 32)
        .ok_or(AgentSessionError::Rejected)?;
    let mut ids = BTreeSet::new();
    for question in questions {
        let id = question["id"]
            .as_str()
            .filter(|id| !id.is_empty())
            .ok_or(AgentSessionError::Rejected)?;
        if !ids.insert(id) {
            return Err(AgentSessionError::Rejected);
        }
    }
    Ok(ids)
}

#[cfg(test)]
mod tests;
