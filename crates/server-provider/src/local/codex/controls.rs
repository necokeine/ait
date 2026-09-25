use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Value, json};
use server_domain::agent_runtime::StoredAgentConfig;

use super::{CodexClient, Transport, native_sessions};
use crate::ports::agent_session::{AgentSessionError, AgentSessionSpec};
use crate::ports::controls::NativeSubagent;

impl CodexClient {
    pub(super) async fn query(
        &self,
        cwd: &str,
        method: &str,
        params: Value,
    ) -> Result<Value, AgentSessionError> {
        let mut transport = Transport::spawn(&self.program, cwd, self.deadline)?;
        let result = async {
            transport.initialize().await?;
            transport.request(method, params).await
        }
        .await;
        transport.close().await?;
        result
    }

    pub(super) async fn validate_remote(
        &self,
        spec: &AgentSessionSpec,
    ) -> Result<(), AgentSessionError> {
        super::validate(spec)?;
        if !fast(&spec.config) {
            return Ok(());
        }
        let details = self.discover_native(&spec.cwd).await?;
        let model = details.models.iter().find(|model| {
            spec.config
                .model
                .as_ref()
                .map_or(model["isDefault"] == true, |id| model["id"] == *id)
        });
        if model.is_some_and(|model| model["supportsFastMode"] == true) {
            Ok(())
        } else {
            Err(AgentSessionError::Rejected)
        }
    }

    pub(super) async fn native_commands(&self, cwd: &str) -> Result<Vec<Value>, AgentSessionError> {
        let response = self
            .query(cwd, "skills/list", json!({"cwds":[cwd],"forceReload":true}))
            .await?;
        Ok(skills(&response,cwd)?.into_iter().map(|skill|json!({"name":skill.name,"description":skill.description,"argumentHint":"","kind":"skill"})).collect())
    }

    pub(super) async fn native_subagents(
        &self,
        cwd: &str,
    ) -> Result<Vec<NativeSubagent>, AgentSessionError> {
        let mut transport = Transport::spawn(&self.program, cwd, self.deadline)?;
        let result = async {
            transport.initialize().await?;
            child_pages(&mut transport).await
        }
        .await;
        transport.close().await?;
        result
    }
}

async fn child_pages(transport: &mut Transport) -> Result<Vec<NativeSubagent>, AgentSessionError> {
    let mut cursor: Option<String> = None;
    let mut seen = BTreeSet::new();
    let mut children = BTreeMap::new();
    loop {
        let response = transport.request("thread/list",json!({"cursor":cursor,"limit":100,
            "sortKey":"updated_at","sortDirection":"desc","sourceKinds":["subAgentThreadSpawn"],"modelProviders":[]})).await?;
        for thread in response["data"]
            .as_array()
            .ok_or(AgentSessionError::Failed)?
        {
            let Some(parent) = native_sessions::parent(thread)? else {
                continue;
            };
            let facts = native_sessions::descriptor(thread)?;
            let status = match thread.pointer("/status/type").and_then(Value::as_str) {
                Some("active") => "running",
                Some("systemError") => "failed",
                Some("idle" | "notLoaded") => "completed",
                _ => return Err(AgentSessionError::Failed),
            };
            let id = facts.provider_handle_id;
            let child = NativeSubagent {
                parent_id: parent,
                cwd: facts.cwd.clone(),
                id: id.clone(),
                descriptor: json!({"id":id,"provider":"codex","title":facts.title.or_else(||thread["agentNickname"].as_str().map(str::to_owned)),
                    "description":facts.first_prompt_preview,"status":status,"createdAt":native_sessions::timestamp(&thread["createdAt"])?,
                    "updatedAt":facts.last_activity_at,"toolCallId":null,"cwd":facts.cwd,"subtitle":thread["agentRole"].as_str()}),
            };
            if let Some(previous) = children.insert(id, child) {
                let current = children
                    .get(&previous.id)
                    .ok_or(AgentSessionError::Failed)?;
                if previous.parent_id != current.parent_id || previous.cwd != current.cwd {
                    return Err(AgentSessionError::Failed);
                }
            }
            if children.len() > 4096 {
                return Err(AgentSessionError::Failed);
            }
        }
        cursor = match &response["nextCursor"] {
            Value::Null => return Ok(children.into_values().collect()),
            Value::String(cursor) if !cursor.is_empty() => Some(cursor.clone()),
            _ => return Err(AgentSessionError::Failed),
        };
        if seen.len() >= 64 || !seen.insert(cursor.clone()) {
            return Err(AgentSessionError::Failed);
        }
    }
}

pub(super) struct Skill {
    pub(super) name: String,
    pub(super) description: String,
    pub(super) path: String,
}

pub(super) fn skills(response: &Value, cwd: &str) -> Result<Vec<Skill>, AgentSessionError> {
    let mut result = BTreeMap::new();
    for entry in response["data"]
        .as_array()
        .ok_or(AgentSessionError::Failed)?
    {
        if entry["cwd"]
            .as_str()
            .and_then(|path| std::fs::canonicalize(path).ok())
            != std::fs::canonicalize(cwd).ok()
        {
            continue;
        }
        if !entry["errors"].as_array().is_some_and(Vec::is_empty) {
            return Err(AgentSessionError::Failed);
        }
        for skill in entry["skills"]
            .as_array()
            .ok_or(AgentSessionError::Failed)?
        {
            if skill["enabled"] != true {
                continue;
            }
            let name = native_sessions::text(skill, "name")?.to_owned();
            let path = native_sessions::text(skill, "path")?.to_owned();
            if !std::path::Path::new(&path).is_absolute() {
                return Err(AgentSessionError::Failed);
            }
            let description = skill["description"]
                .as_str()
                .ok_or(AgentSessionError::Failed)?
                .to_owned();
            result.entry(name.clone()).or_insert(Skill {
                name,
                description,
                path,
            });
        }
    }
    Ok(result.into_values().collect())
}

pub(super) fn fast(config: &StoredAgentConfig) -> bool {
    config
        .feature_values
        .as_ref()
        .and_then(|values| values.get("fast_mode"))
        .is_some_and(|value| value == true)
}

pub(super) fn modes() -> Vec<Value> {
    vec![
        json!({"id":"read-only","label":"Read only","description":"Inspect without writing files"}),
        json!({"id":"auto","label":"Default Permissions","description":"Write in the workspace and request approval when needed"}),
        json!({"id":"full-access","label":"Full Access","description":"Run without sandbox restrictions or approval prompts"}),
    ]
}

pub(super) fn features(config: &StoredAgentConfig) -> Vec<Value> {
    vec![
        json!({"id":"fast_mode","type":"toggle","label":"Fast mode","description":"Use the selected model's fast service tier when available","value":fast(config)}),
    ]
}

pub(super) fn policy(config: &StoredAgentConfig) -> (&'static str, &'static str, Value) {
    match config.mode_id.as_deref().unwrap_or("read-only") {
        "auto" => (
            "on-request",
            "workspace-write",
            json!({"type":"workspaceWrite","networkAccess":false}),
        ),
        "full-access" => (
            "never",
            "danger-full-access",
            json!({"type":"dangerFullAccess"}),
        ),
        _ => (
            "never",
            "read-only",
            json!({"type":"readOnly","networkAccess":false}),
        ),
    }
}

#[cfg(test)]
mod tests;
