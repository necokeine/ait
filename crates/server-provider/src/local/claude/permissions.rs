use serde_json::{Value, json};
use uuid::Uuid;

use super::config::text;
use crate::ports::agent_session::AgentSessionError;

#[derive(Debug)]
pub(super) struct Pending {
    pub(super) agent_id: Option<String>,
    pub(super) native_id: String,
    pub(super) request: Value,
    input: Value,
}

pub(super) fn capture(message: &Value) -> Result<Pending, AgentSessionError> {
    let native_id = text(message, "request_id")?.to_owned();
    let params = &message["request"];
    if params["subtype"] != "can_use_tool"
        || !params["input"].is_object()
        || serde_json::to_vec(params)
            .map_err(|_| AgentSessionError::Failed)?
            .len()
            > 65536
    {
        return Err(AgentSessionError::Failed);
    }
    let name = text(params, "tool_name")?;
    let mut input = params["input"].clone();
    let question = name == "AskUserQuestion";
    if question {
        let questions = input["questions"]
            .as_array_mut()
            .filter(|values| !values.is_empty() && values.len() <= 32)
            .ok_or(AgentSessionError::Failed)?;
        for question in questions {
            text(question, "question")?;
            text(question, "header")?;
            question["allowOther"] = json!(true);
        }
    }
    let suggestions: Vec<Value> = params["permission_suggestions"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|update| valid_update(update))
        .cloned()
        .collect();
    let mut actions = vec![
        json!({"id":"allow","label":"Allow once","behavior":"allow","variant":"primary"}),
        json!({"id":"deny","label":"Deny","behavior":"deny","variant":"secondary"}),
    ];
    for (index, suggestion) in suggestions.iter().enumerate() {
        if suggestion["behavior"] == "allow" && !question {
            let destination = suggestion["destination"].as_str().unwrap_or("session");
            actions.push(json!({"id":format!("allow-update-{index}"),
                "label":format!("Allow rule in {destination}: {}", suggestion["rules"]),
                "behavior":"allow","variant":"secondary"}));
        }
    }
    Ok(Pending {
        agent_id: params["agent_id"].as_str().map(str::to_owned),
        native_id,
        input: params["input"].clone(),
        request: json!({"id":Uuid::new_v4().to_string(),"provider":"claude","name":name,
            "kind":if question {"question"} else if name=="ExitPlanMode" {"plan"} else {"tool"},"input":input,
            "suggestions":suggestions,"actions":actions}),
    })
}

pub(super) fn resolve(pending: &Pending, response: &Value) -> Result<Value, AgentSessionError> {
    let object = response.as_object().ok_or(AgentSessionError::Rejected)?;
    let allow = match response["behavior"].as_str() {
        Some("allow") => true,
        Some("deny") => false,
        _ => return Err(AgentSessionError::Rejected),
    };
    let fields = if allow {
        &[
            "behavior",
            "selectedActionId",
            "updatedInput",
            "updatedPermissions",
        ][..]
    } else {
        &["behavior", "selectedActionId", "message", "interrupt"][..]
    };
    if object.keys().any(|key| !fields.contains(&key.as_str()))
        || response.get("selectedActionId").is_some_and(|id| {
            !pending.request["actions"]
                .as_array()
                .is_some_and(|actions| {
                    actions.iter().any(|action| {
                        action["id"] == *id && action["behavior"] == response["behavior"]
                    })
                })
        })
        || response
            .get("message")
            .is_some_and(|value| !value.is_string())
        || response
            .get("interrupt")
            .is_some_and(|value| !value.is_boolean())
    {
        return Err(AgentSessionError::Rejected);
    }
    let mut result = if allow {
        let input = if pending.request["kind"] == "question" {
            question_input(&pending.input, &response["updatedInput"])?
        } else {
            let input = response.get("updatedInput").unwrap_or(&pending.input);
            if !input.is_object()
                || serde_json::to_vec(input)
                    .map_err(|_| AgentSessionError::Rejected)?
                    .len()
                    > 65536
            {
                return Err(AgentSessionError::Rejected);
            }
            input.clone()
        };
        json!({"behavior":"allow","updatedInput":input})
    } else {
        json!({"behavior":"deny","message":response["message"].as_str().unwrap_or("Denied by user"),
            "interrupt":response["interrupt"].as_bool().unwrap_or(false)})
    };
    if allow {
        let selected = response["selectedActionId"]
            .as_str()
            .and_then(|id| id.strip_prefix("allow-update-"))
            .and_then(|index| index.parse::<usize>().ok())
            .and_then(|index| pending.request["suggestions"].get(index));
        if selected.is_some() && response.get("updatedPermissions").is_some() {
            return Err(AgentSessionError::Rejected);
        }
        let updates = selected
            .map(|update| vec![update.clone()])
            .or_else(|| response["updatedPermissions"].as_array().cloned());
        if response.get("updatedPermissions").is_some() && updates.is_none() {
            return Err(AgentSessionError::Rejected);
        }
        if let Some(updates) = updates {
            if updates.len() > 128
                || updates.iter().any(|update| !valid_update(update))
                || pending.request["kind"] == "question"
            {
                return Err(AgentSessionError::Rejected);
            }
            result["updatedPermissions"] = json!(updates);
        }
    }
    Ok(
        json!({"type":"control_response","response":{"subtype":"success",
        "request_id":pending.native_id,"response":result}}),
    )
}

fn valid_update(value: &Value) -> bool {
    let Some(fields) = value.as_object() else {
        return false;
    };
    if fields
        .keys()
        .any(|key| !matches!(key.as_str(), "type" | "rules" | "behavior" | "destination"))
        || !matches!(
            value["type"].as_str(),
            Some("addRules" | "replaceRules" | "removeRules")
        )
        || !matches!(value["behavior"].as_str(), Some("allow" | "ask" | "deny"))
        || !matches!(
            value["destination"].as_str(),
            Some("session" | "userSettings" | "projectSettings" | "localSettings")
        )
    {
        return false;
    }
    value["rules"].as_array().is_some_and(|rules| {
        rules.len() <= 128
            && rules.iter().all(|rule| {
                rule.as_object().is_some_and(|fields| {
                    fields
                        .keys()
                        .all(|key| matches!(key.as_str(), "toolName" | "ruleContent"))
                }) && rule["toolName"].as_str().is_some_and(|name| {
                    !name.is_empty() && name.len() <= 256 && !name.chars().any(char::is_control)
                }) && rule.get("ruleContent").is_none_or(|content| {
                    content
                        .as_str()
                        .is_some_and(|content| content.len() <= 4096 && !content.contains('\0'))
                })
            })
    })
}

fn question_input(original: &Value, updated: &Value) -> Result<Value, AgentSessionError> {
    let updated = updated.as_object().ok_or(AgentSessionError::Rejected)?;
    for (key, value) in updated {
        if key == "answers" {
            continue;
        }
        let mut value = value.clone();
        if key == "questions"
            && let Some(questions) = value.as_array_mut()
        {
            for question in questions {
                if let Some(question) = question.as_object_mut() {
                    question.remove("allowOther");
                }
            }
        }
        if original.get(key) != Some(&value) {
            return Err(AgentSessionError::Rejected);
        }
    }
    let answers = updated
        .get("answers")
        .and_then(Value::as_object)
        .ok_or(AgentSessionError::Rejected)?;
    let questions = original["questions"]
        .as_array()
        .ok_or(AgentSessionError::Rejected)?;
    if answers.len() != questions.len() {
        return Err(AgentSessionError::Rejected);
    }
    let mut normalized = serde_json::Map::new();
    for question in questions {
        let title = text(question, "question")?;
        let header = text(question, "header")?;
        let answer = answers
            .get(title)
            .or_else(|| answers.get(header))
            .and_then(Value::as_str)
            .filter(|answer| !answer.is_empty() && answer.len() <= 4096)
            .ok_or(AgentSessionError::Rejected)?;
        normalized.insert(title.to_owned(), json!(answer));
    }
    let mut input = original.clone();
    input["answers"] = Value::Object(normalized);
    Ok(input)
}
