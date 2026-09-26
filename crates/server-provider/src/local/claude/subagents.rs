//! Verified Claude child transcripts. A parent Task declaration is required for every link.

pub(super) mod live;
mod workflow;

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::{Value, json};
use server_domain::agent_runtime::AgentPersistenceHandle;
use uuid::Uuid;

use super::{ClaudeClient, history};
use crate::ports::agent_session::AgentSessionError;
use crate::ports::controls::NativeSubagent;
use crate::ports::native_history::SessionHistory;

pub(super) const LOCATOR: &str = "claudeSubagent";

struct Child {
    native_id: String,
    call: Option<String>,
    meta: Value,
    records: Vec<Value>,
}

struct Declaration {
    owner: String,
    input: Value,
}

pub(super) fn list(
    client: &ClaudeClient,
    cwd: &str,
) -> Result<Vec<NativeSubagent>, AgentSessionError> {
    let directory = history::project_dir(client, cwd)?;
    let entries = match std::fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Err(_) => return Err(AgentSessionError::Failed),
    };
    let mut result = Vec::new();
    for (count, entry) in entries.enumerate() {
        if count >= 16384 {
            return Err(AgentSessionError::Failed);
        }
        let entry = entry.map_err(|_| AgentSessionError::Failed)?;
        let Some(id) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if Uuid::parse_str(&id).is_err() || !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            continue;
        }
        for (child, _) in tree(client, cwd, &id)? {
            result.push(child);
        }
        if result.len() > 4096 {
            return Err(AgentSessionError::Failed);
        }
    }
    Ok(result)
}

pub(super) fn read(
    client: &ClaudeClient,
    handle: &AgentPersistenceHandle,
    cwd: &str,
) -> Result<SessionHistory, AgentSessionError> {
    if handle.provider != "claude" {
        return Err(AgentSessionError::Rejected);
    }
    let locator = handle
        .metadata
        .as_ref()
        .and_then(|metadata| metadata.get(LOCATOR))
        .ok_or(AgentSessionError::Rejected)?;
    let root = locator["root"]
        .as_str()
        .filter(|id| Uuid::parse_str(id).is_ok())
        .ok_or(AgentSessionError::Rejected)?;
    let root_cwd = locator["cwd"].as_str().ok_or(AgentSessionError::Rejected)?;
    let (_, history) = tree(client, root_cwd, root)?
        .into_iter()
        .find(|(child, _)| child.id == handle.session_id)
        .ok_or(AgentSessionError::Unavailable)?;
    if std::fs::canonicalize(&history.descriptor.cwd).ok() != std::fs::canonicalize(cwd).ok() {
        return Err(AgentSessionError::Rejected);
    }
    Ok(history)
}

fn tree(
    client: &ClaudeClient,
    cwd: &str,
    root: &str,
) -> Result<Vec<(NativeSubagent, SessionHistory)>, AgentSessionError> {
    let directory = history::project_dir(client, cwd)?;
    let parent = history::read_active_records(&directory.join(format!("{root}.jsonl")))
        .map_err(|_| AgentSessionError::Failed)?;
    let child_root = directory
        .join(root)
        .symlink_metadata()
        .map_err(|_| AgentSessionError::Failed)?;
    if !child_root.is_dir() || child_root.file_type().is_symlink() {
        return Err(AgentSessionError::Rejected);
    }
    let parent_history = history::parse(&parent, root, &client.images)?;
    if std::fs::canonicalize(&parent_history.descriptor.cwd).ok() != std::fs::canonicalize(cwd).ok()
    {
        return Err(AgentSessionError::Rejected);
    }
    let mut children = read_children(&directory.join(root).join("subagents"), root)?;
    let mut declarations = BTreeMap::new();
    let mut legacy = BTreeMap::new();
    collect_declarations(&parent, root, false, &mut declarations, &mut legacy)?;
    for child in &children {
        collect_declarations(
            &child.records,
            &child.native_id,
            true,
            &mut declarations,
            &mut legacy,
        )?;
    }
    for child in &mut children {
        child.call = child.meta["toolUseId"]
            .as_str()
            .filter(|call| declarations.contains_key(*call))
            .map(str::to_owned)
            .or_else(|| {
                legacy
                    .get(&child.native_id)
                    .cloned()
                    .filter(|call| declarations.contains_key(call))
            });
    }
    let identities: BTreeMap<_, _> = children
        .iter()
        .filter_map(|child| {
            child
                .call
                .as_ref()
                .map(|call| (child.native_id.clone(), call.clone()))
        })
        .collect();
    let mut result = workflow::histories(client, (cwd, root), &parent_history, &parent)?;
    for child in children {
        let Some(call) = &child.call else {
            continue;
        };
        let declaration = declarations.get(call).ok_or(AgentSessionError::Failed)?;
        let parent = if declaration.owner == root {
            root.to_owned()
        } else {
            let Some(parent) = identities.get(&declaration.owner) else {
                continue;
            };
            parent.clone()
        };
        result.push(project(client, (cwd, root), child, declaration, &parent)?);
    }
    Ok(result)
}

fn read_children(directory: &Path, root: &str) -> Result<Vec<Child>, AgentSessionError> {
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Err(_) => return Err(AgentSessionError::Failed),
    };
    if directory
        .symlink_metadata()
        .map_err(|_| AgentSessionError::Failed)?
        .file_type()
        .is_symlink()
    {
        return Err(AgentSessionError::Rejected);
    }
    let mut result = Vec::new();
    for (count, entry) in entries.enumerate() {
        if count >= 8192 {
            return Err(AgentSessionError::Failed);
        }
        let entry = entry.map_err(|_| AgentSessionError::Failed)?;
        let name = entry.file_name();
        let Some(id) = name
            .to_str()
            .and_then(|name| name.strip_prefix("agent-"))
            .and_then(|name| name.strip_suffix(".jsonl"))
        else {
            continue;
        };
        if !valid_component(id) || !entry.file_type().is_ok_and(|kind| kind.is_file()) {
            continue;
        }
        let records =
            history::read_active_records(&entry.path()).map_err(|_| AgentSessionError::Failed)?;
        if records.iter().any(|record| {
            record["sessionId"].as_str().is_some_and(|id| id != root)
                || record["agentId"].as_str().is_some_and(|agent| agent != id)
        }) {
            return Err(AgentSessionError::Rejected);
        }
        let path = directory.join(format!("agent-{id}.meta.json"));
        let meta = if path.symlink_metadata().is_ok_and(|meta| {
            meta.is_file() && !meta.file_type().is_symlink() && meta.len() <= 16384
        }) {
            std::fs::read(path)
                .ok()
                .and_then(|bytes| serde_json::from_slice(&bytes).ok())
                .unwrap_or(Value::Null)
        } else {
            Value::Null
        };
        result.push(Child {
            native_id: id.to_owned(),
            call: None,
            meta,
            records,
        });
    }
    Ok(result)
}

fn collect_declarations(
    records: &[Value],
    owner: &str,
    sidechain: bool,
    declarations: &mut BTreeMap<String, Declaration>,
    legacy: &mut BTreeMap<String, String>,
) -> Result<(), AgentSessionError> {
    for record in records {
        if !sidechain && record["isSidechain"] == true {
            continue;
        }
        for block in record["message"]["content"]
            .as_array()
            .into_iter()
            .flatten()
        {
            if block["type"] == "tool_use"
                && matches!(block["name"].as_str(), Some("Agent" | "Task" | "Workflow"))
            {
                let call = block["id"]
                    .as_str()
                    .filter(|call| valid_component(call))
                    .ok_or(AgentSessionError::Failed)?;
                if let Some(old) = declarations.insert(
                    call.to_owned(),
                    Declaration {
                        owner: owner.to_owned(),
                        input: block["input"].clone(),
                    },
                ) && old.owner != owner
                {
                    return Err(AgentSessionError::Failed);
                }
            }
            if block["type"] == "tool_result"
                && let Some(call) = block["tool_use_id"].as_str()
            {
                let id = record["toolUseResult"]["agentId"].as_str().or_else(|| {
                    block["content"]
                        .as_str()
                        .and_then(|text| {
                            text.lines()
                                .find_map(|line| line.trim().strip_prefix("agentId: "))
                        })
                        .and_then(|line| line.split_whitespace().next())
                });
                if let Some(id) = id.filter(|id| valid_component(id)) {
                    legacy.insert(id.to_owned(), call.to_owned());
                }
            }
        }
    }
    Ok(())
}

fn project(
    client: &ClaudeClient,
    binding: (&str, &str),
    child: Child,
    declaration: &Declaration,
    parent: &str,
) -> Result<(NativeSubagent, SessionHistory), AgentSessionError> {
    let (cwd, root) = binding;
    let call = child.call.ok_or(AgentSessionError::Failed)?;
    let status = child.records.iter().rev().find_map(|record| {
        if record["type"] == "result" {
            Some(if record["is_error"] == true {
                "failed"
            } else {
                "completed"
            })
        } else if record["type"] == "system" && record["subtype"] == "task_notification" {
            match record["status"].as_str() {
                Some("failed") => Some("failed"),
                Some("stopped" | "killed") => Some("canceled"),
                _ => None,
            }
        } else {
            None
        }
    });
    let mut records = child.records;
    for record in &mut records {
        record["isSidechain"] = json!(false);
        record["parent_tool_use_id"] = Value::Null;
        record["sessionId"] = json!(call);
        if record["cwd"].is_null() {
            record["cwd"] = json!(cwd);
        }
        if record["type"] == "assistant" && record["message"]["id"].is_null() {
            record["message"]["id"] = record["uuid"].clone();
        }
    }
    let mut history = history::parse(&records, &call, &client.images)?;
    history.parent_id = Some(parent.to_owned());
    let descriptor = json!({"id":call,"provider":"claude",
        "title":declaration.input["name"].as_str().or_else(||child.meta["agentType"].as_str()).or_else(||declaration.input["subagent_type"].as_str()),
        "description":child.meta["description"].as_str().or_else(||declaration.input["description"].as_str()),
        "status":status.unwrap_or(if history.active {"running"} else {"completed"}),"createdAt":history.created_at,
        "updatedAt":history.descriptor.last_activity_at,"toolCallId":call,"cwd":history.descriptor.cwd,
        "subtitle":history.config.model});
    let locator = BTreeMap::from([(
        LOCATOR.to_owned(),
        json!({"root":root,"cwd":cwd,"nativeId":child.native_id}),
    )]);
    Ok((
        NativeSubagent {
            persistence: Some(AgentPersistenceHandle {
                provider: "claude".into(),
                session_id: call.clone(),
                native_handle: None,
                metadata: Some(locator),
            }),
            id: call,
            parent_id: parent.to_owned(),
            cwd: history.descriptor.cwd.clone(),
            descriptor,
        },
        history,
    ))
}

fn valid_component(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

#[cfg(test)]
mod tests;
