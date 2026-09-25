use std::collections::{BTreeMap, BTreeSet};

use chrono::Utc;
use serde_json::{Value, json};

use super::{CodexClient, Transport};
use crate::ports::agent_session::AgentSessionError;
use crate::protocol::provider::Details;
use crate::protocol::timeline::NativeItem;

impl CodexClient {
    pub(super) async fn discover_native(&self, cwd: &str) -> Result<Details, AgentSessionError> {
        let mut transport = Transport::spawn(&self.program, cwd, self.deadline)?;
        let result = async {
            transport.initialize().await?;
            let mut models = BTreeMap::new();
            let mut cursor: Option<String> = None;
            let mut seen = BTreeSet::new();
            loop {
                let page = transport
                    .request(
                        "model/list",
                        json!({"cursor":cursor,"limit":100,"includeHidden":false}),
                    )
                    .await?;
                let entries = page["data"].as_array().ok_or(AgentSessionError::Failed)?;
                for entry in entries {
                    let model = model(entry)?;
                    let id = model["id"]
                        .as_str()
                        .ok_or(AgentSessionError::Failed)?
                        .to_owned();
                    models.insert(id, model);
                }
                if models.len() > 4096 || seen.len() > 64 {
                    return Err(AgentSessionError::Failed);
                }
                cursor = match &page["nextCursor"] {
                    Value::Null => None,
                    Value::String(cursor) if !cursor.is_empty() => Some(cursor.clone()),
                    _ => return Err(AgentSessionError::Failed),
                };
                let Some(cursor) = &cursor else {
                    break;
                };
                if !seen.insert(cursor.clone()) {
                    return Err(AgentSessionError::Failed);
                }
            }
            Ok(Details {
                models: models.into_values().collect(),
                modes: super::controls::modes(),
                features: super::controls::features(
                    &server_domain::agent_runtime::StoredAgentConfig::default(),
                ),
            })
        }
        .await;
        let closed = transport.close().await;
        closed?;
        result
    }

    pub(super) async fn read_history(
        &self,
        id: &str,
        cwd: &str,
    ) -> Result<Vec<NativeItem>, AgentSessionError> {
        self.inspect_native(id, cwd)
            .await
            .map(|history| history.entries)
    }
}

fn model(native: &Value) -> Result<Value, AgentSessionError> {
    let id = native["model"]
        .as_str()
        .filter(|id| !id.is_empty())
        .ok_or(AgentSessionError::Failed)?;
    let label = native["displayName"]
        .as_str()
        .ok_or(AgentSessionError::Failed)?;
    let efforts = native["supportedReasoningEfforts"]
        .as_array()
        .ok_or(AgentSessionError::Failed)?;
    let options = efforts
        .iter()
        .map(|effort| {
            let id = effort["reasoningEffort"]
                .as_str()
                .ok_or(AgentSessionError::Failed)?;
            let mut option = json!({"id":id,"label":id});
            if let Some(description) = effort["description"].as_str() {
                option["description"] = json!(description);
            }
            Ok(option)
        })
        .collect::<Result<Vec<_>, AgentSessionError>>()?;
    let mut value = json!({"provider":"codex","id":id,"label":label,
        "isSelectable":native["hidden"].as_bool()!=Some(true),"isDefault":native["isDefault"].as_bool().unwrap_or(false),"thinkingOptions":options});
    value["supportsFastMode"] = json!(
        native["serviceTiers"]
            .as_array()
            .is_some_and(|tiers| tiers.iter().any(|tier| tier["id"] == "fast"))
            || native["additionalSpeedTiers"]
                .as_array()
                .is_some_and(|tiers| tiers.iter().any(|tier| tier == "fast"))
    );
    if let Some(description) = native["description"].as_str() {
        value["description"] = json!(description);
    }
    if let Some(effort) = native["defaultReasoningEffort"].as_str() {
        value["defaultThinkingOptionId"] = json!(effort);
    }
    Ok(value)
}

pub(super) fn timeline_item(
    native: &Value,
    turn: &str,
    timestamp: &str,
) -> Result<Option<NativeItem>, AgentSessionError> {
    let kind = native["type"].as_str().ok_or(AgentSessionError::Failed)?;
    let id = native["id"]
        .as_str()
        .filter(|id| !id.is_empty())
        .ok_or(AgentSessionError::Failed)?;
    let item = match kind {
        "userMessage" => {
            let content = native["content"]
                .as_array()
                .ok_or(AgentSessionError::Failed)?;
            let text = content
                .iter()
                .filter_map(|part| part["text"].as_str())
                .collect::<Vec<_>>()
                .join("\n");
            json!({"type":"user_message","text":text,"messageId":id})
        }
        "agentMessage" => {
            json!({"type":"assistant_message","text":native["text"].as_str().ok_or(AgentSessionError::Failed)?,"messageId":id})
        }
        "reasoning" => {
            let text = native["summary"]
                .as_array()
                .or_else(|| native["content"].as_array())
                .map(|values| {
                    values
                        .iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join("\n")
                })
                .unwrap_or_default();
            json!({"type":"reasoning","text":text})
        }
        "contextCompaction" => json!({"type":"compaction","status":"completed"}),
        "plan" => {
            json!({"type":"notification","level":"info","message":native["text"].as_str().unwrap_or("")})
        }
        _ => {
            if native["status"] == "inProgress" {
                return Ok(None);
            }
            let failed = native["status"] == "failed" || native["status"] == "declined";
            let error = failed.then_some("Native tool failed");
            // Preserve unknown native tool content in Paseo's explicit generic tool detail.
            json!({"type":"tool_call","callId":id,"name":kind,"status":if failed {"failed"} else {"completed"},
                "error":error,"detail":{"type":"unknown","input":native,"output":null}})
        }
    };
    Ok(Some(NativeItem {
        key: format!("native:{turn}:{id}"),
        turn_id: Some(turn.to_owned()),
        timestamp: timestamp.to_owned(),
        item,
    }))
}

pub(super) fn timestamp() -> String {
    Utc::now().to_rfc3339()
}

#[cfg(test)]
mod tests;
