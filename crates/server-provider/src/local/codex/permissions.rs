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
    Mcp,
}

pub(super) fn supported(method: &str) -> bool {
    matches!(
        method,
        "item/commandExecution/requestApproval"
            | "item/fileChange/requestApproval"
            | "item/tool/requestUserInput"
            | "mcpServer/elicitation/request"
    )
}

impl CodexSession {
    pub(super) fn resolve_native_permission(&mut self, native: &Value) -> Option<String> {
        let id = self
            .permissions
            .iter()
            .find(|(_, pending)| &pending.native_id == native)?
            .0
            .clone();
        self.permissions.remove(&id);
        Some(id)
    }

    pub(super) fn capture_permission(
        &mut self,
        message: &Value,
    ) -> Result<AgentTurnEvent, AgentSessionError> {
        let params = &message["params"];
        let mcp = message["method"] == "mcpServer/elicitation/request";
        let child = params["threadId"]
            .as_str()
            .is_some_and(|id| self.children.contains(id));
        if (self.active_turn.is_none() && !mcp && !child)
            || (params["threadId"] != self.id && !child)
            || !child
                && (!mcp || !params["turnId"].is_null())
                && params["turnId"].as_str() != self.active_turn.as_deref()
            || self.permissions.len() >= 32
            || self
                .permissions
                .values()
                .any(|pending| pending.native_id == message["id"])
            || !(message["id"].is_string() || message["id"].as_i64().is_some())
            || (!mcp && !params["itemId"].is_string())
            || (!mcp && !params["turnId"].is_string())
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
            Some("mcpServer/elicitation/request") => (Kind::Mcp, "McpElicitation", "tool"),
            Some("item/tool/requestUserInput") if params["isBlocking"] != false => {
                (Kind::Question, "request_user_input", "question")
            }
            _ => return Err(AgentSessionError::Unavailable),
        };
        if kind == Kind::Question {
            question_ids(&params["questions"])?;
        }
        let id = Uuid::new_v4().to_string();
        let mut request = json!({"id":id,"provider":"codex","name":name,"kind":label,"input":params,
            "actions":[{"id":"allow","label":"Allow once","behavior":"allow","variant":"primary"},
                {"id":"deny","label":"Deny","behavior":"deny","variant":"secondary"}]});
        if kind == Kind::Mcp {
            if !params["serverName"].is_string() || !params["message"].is_string() {
                return Err(AgentSessionError::Failed);
            }
            let questions = crate::local::elicitation::questions(&params["requestedSchema"])?;
            request["title"] = json!(params["message"]);
            if !questions.is_empty() {
                request["kind"] = json!("question");
                request["input"]["questions"] = json!(questions);
            }
        }
        let actions = request["actions"]
            .as_array_mut()
            .ok_or(AgentSessionError::Failed)?;
        if matches!(kind, Kind::Command | Kind::File) {
            actions.push(json!({"id":"allow-session","label":"Allow for this session","behavior":"allow","variant":"secondary"}));
        }
        if kind == Kind::Command {
            if let Some(prefix) = params["proposedExecpolicyAmendment"]
                .as_array()
                .filter(|prefix| !prefix.is_empty() && prefix.iter().all(Value::is_string))
            {
                actions.push(json!({"id":"allow-prefix","label":format!("Always allow command prefix: {}", json!(prefix)),"behavior":"allow","variant":"secondary"}));
            }
            for (index, amendment) in params["proposedNetworkPolicyAmendments"]
                .as_array()
                .into_iter()
                .flatten()
                .enumerate()
            {
                if matches!(amendment["action"].as_str(), Some("allow" | "deny"))
                    && amendment["host"].is_string()
                {
                    actions.push(json!({"id":format!("network-{index}"),"label":format!("{} network access to {}", amendment["action"].as_str().unwrap_or(""),amendment["host"].as_str().unwrap_or("")),
                        "behavior":amendment["action"],"variant":"secondary"}));
                }
            }
        }
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
        || response.get("selectedActionId").is_some_and(|id| {
            if id == if allow { "allow" } else { "deny" } {
                return false;
            }
            !pending.request["actions"]
                .as_array()
                .is_some_and(|actions| {
                    actions.iter().any(|action| {
                        action["id"] == *id && action["behavior"] == response["behavior"]
                    })
                })
        })
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
        return question_resolution(pending, response, allow);
    }
    if pending.kind == Kind::Mcp {
        return Ok(
            json!({"action":if allow {"accept"} else if response["interrupt"] == true {"cancel"} else {"decline"},
            "content":if allow {crate::local::elicitation::content(&pending.request["input"]["requestedSchema"], &response["updatedInput"])?} else {Value::Null},"_meta":null}),
        );
    }
    if response.get("updatedInput").is_some() {
        return Err(AgentSessionError::Rejected);
    }
    // Do not turn a one-call decision into a session-wide filesystem permission grant.
    let selected = response["selectedActionId"].as_str();
    if allow
        && pending.kind == Kind::File
        && !pending.request["input"]["grantRoot"].is_null()
        && selected != Some("allow-session")
    {
        return Err(AgentSessionError::Rejected);
    }
    let decision = match selected {
        Some("allow-session") if allow => json!("acceptForSession"),
        Some("allow-prefix") if allow && pending.kind == Kind::Command => {
            json!({"acceptWithExecpolicyAmendment":{"execpolicy_amendment":pending.request["input"]["proposedExecpolicyAmendment"]}})
        }
        Some(action) if action.starts_with("network-") && pending.kind == Kind::Command => {
            let amendment = action
                .strip_prefix("network-")
                .and_then(|index| index.parse::<usize>().ok())
                .and_then(|index| {
                    pending.request["input"]["proposedNetworkPolicyAmendments"].get(index)
                })
                .ok_or(AgentSessionError::Rejected)?;
            json!({"applyNetworkPolicyAmendment":{"network_policy_amendment":amendment}})
        }
        _ => json!(if allow { "accept" } else { "decline" }),
    };
    Ok(json!({"decision":decision}))
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

fn question_resolution(
    pending: &Pending,
    response: &Value,
    allow: bool,
) -> Result<Value, AgentSessionError> {
    if !allow {
        return Ok(json!({"answers":{}}));
    }
    if response["updatedInput"].as_object().is_some_and(|input| {
        input.iter().any(|(key, value)| {
            key != "answers" && pending.request["input"].get(key) != Some(value)
        })
    }) {
        return Err(AgentSessionError::Rejected);
    }
    let supplied = response["updatedInput"]["answers"]
        .as_object()
        .ok_or(AgentSessionError::Rejected)?;
    let expected = question_ids(&pending.request["input"]["questions"])?;
    if supplied.len() != expected.len() {
        return Err(AgentSessionError::Rejected);
    }
    let mut answers = serde_json::Map::new();
    let mut used = BTreeSet::new();
    for question in pending.request["input"]["questions"]
        .as_array()
        .ok_or(AgentSessionError::Rejected)?
    {
        let id = question["id"].as_str().ok_or(AgentSessionError::Rejected)?;
        let (key, value) = [
            Some(id),
            question["header"].as_str(),
            question["question"].as_str(),
        ]
        .into_iter()
        .flatten()
        .find_map(|key| supplied.get_key_value(key))
        .ok_or(AgentSessionError::Rejected)?;
        if !used.insert(key) {
            return Err(AgentSessionError::Rejected);
        }
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
        answers.insert(id.to_owned(), json!({"answers":values}));
    }
    Ok(json!({"answers":answers}))
}
