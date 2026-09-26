use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use chrono::DateTime;
use serde_json::{Value, json};

use super::{CodexClient, Transport, discovery, validate_config};
use crate::ports::agent_session::AgentSessionError;
use crate::ports::native_history::{ListOptions, SessionDescriptor, SessionHistory};

impl CodexClient {
    pub(super) async fn list_native(
        &self,
        options: &ListOptions,
    ) -> Result<Vec<SessionDescriptor>, AgentSessionError> {
        if !(1..=4096).contains(&options.scan_limit) {
            return Err(AgentSessionError::Rejected);
        }
        let cwd = options.cwd.clone().map_or_else(
            || std::env::current_dir().map_err(|_| AgentSessionError::Failed),
            |cwd| Ok(cwd.into()),
        )?;
        let cwd = cwd.to_str().ok_or(AgentSessionError::Failed)?;
        let mut transport = Transport::spawn(&self.program, cwd, self.deadline)?;
        let result = async {
            transport.initialize().await?;
            list_pages(&mut transport, options).await
        }
        .await;
        transport.close().await?;
        result
    }

    pub(super) async fn inspect_native(
        &self,
        id: &str,
        cwd: &str,
    ) -> Result<SessionHistory, AgentSessionError> {
        let mut transport = Transport::spawn(&self.program, cwd, self.deadline)?;
        let result = async {
            transport.initialize().await?;
            let response = transport
                .request("thread/read", json!({"threadId":id,"includeTurns":true}))
                .await?;
            let history = history_with_images(&response["thread"], &self.images)?;
            if history.descriptor.provider_handle_id != id
                || Path::new(&history.descriptor.cwd).canonicalize().ok()
                    != Some(
                        Path::new(cwd)
                            .canonicalize()
                            .map_err(|_| AgentSessionError::Failed)?,
                    )
            {
                return Err(AgentSessionError::Failed);
            }
            Ok(history)
        }
        .await;
        transport.close().await?;
        result
    }
}

async fn list_pages(
    transport: &mut Transport,
    options: &ListOptions,
) -> Result<Vec<SessionDescriptor>, AgentSessionError> {
    let mut cursor: Option<String> = None;
    let mut seen = BTreeSet::new();
    let mut sessions = BTreeMap::new();
    loop {
        let page = transport
            .request(
                "thread/list",
                json!({"cursor":cursor,"limit":100,"sortKey":"updated_at",
                    "sortDirection":"desc","cwd":options.cwd,"archived":false,
                    "modelProviders":[],"sourceKinds":[]}),
            )
            .await?;
        let entries = page["data"].as_array().ok_or(AgentSessionError::Failed)?;
        for entry in entries {
            if entry["ephemeral"] == true || parent(entry)?.is_some() {
                continue;
            }
            let descriptor = descriptor(entry)?;
            sessions
                .entry(descriptor.provider_handle_id.clone())
                .or_insert(descriptor);
            if sessions.len() >= options.scan_limit {
                return Ok(sessions.into_values().collect());
            }
        }
        cursor = match &page["nextCursor"] {
            Value::Null => return Ok(sessions.into_values().collect()),
            Value::String(value) if !value.is_empty() => Some(value.clone()),
            _ => return Err(AgentSessionError::Failed),
        };
        if seen.len() >= 64 || !seen.insert(cursor.clone()) {
            return Err(AgentSessionError::Failed);
        }
    }
}

pub(super) fn descriptor(thread: &Value) -> Result<SessionDescriptor, AgentSessionError> {
    let id = text(thread, "id")?;
    let cwd = text(thread, "cwd")?;
    if !Path::new(cwd).is_absolute() {
        return Err(AgentSessionError::Failed);
    }
    Ok(SessionDescriptor {
        provider_id: "codex".to_owned(),
        provider_label: "Codex".to_owned(),
        provider_handle_id: id.to_owned(),
        cwd: cwd.to_owned(),
        title: nonempty(thread["name"].as_str()),
        first_prompt_preview: nonempty(thread["preview"].as_str()),
        last_prompt_preview: None,
        last_activity_at: timestamp(&thread["updatedAt"])?,
    })
}

#[cfg(test)]
pub(super) fn history(thread: &Value) -> Result<SessionHistory, AgentSessionError> {
    history_with_images(thread, &crate::local::images::ImageStore::default())
}

pub(super) fn history_with_images(
    thread: &Value,
    images: &crate::local::images::ImageStore,
) -> Result<SessionHistory, AgentSessionError> {
    let descriptor = descriptor(thread)?;
    let created_at = timestamp(&thread["createdAt"])?;
    let config = server_domain::agent_runtime::StoredAgentConfig {
        model: nonempty(thread["model"].as_str()),
        thinking_option_id: nonempty(thread["reasoningEffort"].as_str()),
        mode_id: Some("read-only".to_owned()),
        ..Default::default()
    };
    validate_config(&config)?;
    let mut active = match thread.pointer("/status/type").and_then(Value::as_str) {
        Some("idle" | "notLoaded") => false,
        Some("active") => true,
        _ => return Err(AgentSessionError::Failed),
    };
    let turns = thread["turns"]
        .as_array()
        .ok_or(AgentSessionError::Failed)?;
    let mut entries = Vec::new();
    let mut identities = BTreeSet::new();
    for turn in turns {
        if turn["status"] == "inProgress" {
            active = true;
            continue;
        }
        if !matches!(
            turn["status"].as_str(),
            Some("completed" | "interrupted" | "failed")
        ) || turn.get("itemsView").is_some_and(|view| view != "full")
        {
            return Err(AgentSessionError::Failed);
        }
        let id = text(turn, "id")?;
        let started = if turn["startedAt"].is_null() {
            created_at.clone()
        } else {
            timestamp(&turn["startedAt"])?
        };
        for item in turn["items"].as_array().ok_or(AgentSessionError::Failed)? {
            for entry in discovery::timeline_items(item, id, &started, images)? {
                if !identities.insert(entry.key.clone()) {
                    return Err(AgentSessionError::Failed);
                }
                entries.push(entry);
            }
        }
    }
    Ok(SessionHistory {
        resume_metadata: BTreeMap::new(),
        parent_id: parent(thread)?,
        descriptor,
        created_at,
        config,
        active,
        entries,
    })
}

pub(super) fn timestamp(value: &Value) -> Result<String, AgentSessionError> {
    DateTime::from_timestamp(value.as_i64().ok_or(AgentSessionError::Failed)?, 0)
        .map(|timestamp| timestamp.to_rfc3339())
        .ok_or(AgentSessionError::Failed)
}

pub(super) fn text<'a>(value: &'a Value, field: &str) -> Result<&'a str, AgentSessionError> {
    value[field]
        .as_str()
        .filter(|value| {
            !value.trim().is_empty() && value.len() <= 4096 && !value.chars().any(char::is_control)
        })
        .ok_or(AgentSessionError::Failed)
}

pub(super) fn parent(thread: &Value) -> Result<Option<String>, AgentSessionError> {
    let value = thread
        .get("parentThreadId")
        .filter(|value| !value.is_null())
        .or_else(|| thread.pointer("/source/subAgent/thread_spawn/parent_thread_id"));
    value
        .map(|value| {
            value
                .as_str()
                .filter(|id| !id.is_empty() && id.len() <= 512 && !id.chars().any(char::is_control))
                .map(str::to_owned)
                .ok_or(AgentSessionError::Failed)
        })
        .transpose()
}

fn nonempty(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

#[cfg(test)]
mod tests;
