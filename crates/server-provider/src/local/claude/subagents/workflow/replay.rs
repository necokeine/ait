use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::path::Path;

use crate::local::claude::{ClaudeClient, history, streaming};
use crate::ports::agent_session::{AgentSessionError, AgentTurnEvent};
use crate::ports::controls::NativeSubagent;
use crate::ports::native_history::{SessionDescriptor, SessionHistory};
use serde_json::{Value, json};
use server_domain::agent_runtime::{AgentPersistenceHandle, StoredAgentConfig};

pub(in crate::local::claude::subagents) fn histories(
    client: &ClaudeClient,
    context: (&str, &str),
    parent: &SessionHistory,
    records: &[Value],
) -> Result<Vec<(NativeSubagent, SessionHistory)>, AgentSessionError> {
    let (cwd, root) = context;
    let directory = history::project_dir(client, cwd)?.join(root);
    let links = links(records);
    let mut histories = Vec::new();
    for (run, call) in links {
        let path = directory.join("workflows").join(format!("{run}.json"));
        let Some(summary) = summary(&path).filter(|summary| summary["runId"] == run) else {
            continue;
        };
        histories.push(project(
            client,
            (&directory, &run, &call),
            parent,
            &summary,
        )?);
    }
    Ok(histories)
}

fn links(records: &[Value]) -> BTreeMap<String, String> {
    let mut declarations = BTreeSet::new();
    let blocks = || {
        records
            .iter()
            .filter(|record| record["isSidechain"] != true)
            .flat_map(|record| {
                record["message"]["content"]
                    .as_array()
                    .into_iter()
                    .flatten()
            })
    };
    for block in blocks() {
        if block["type"] == "tool_use"
            && block["name"] == "Workflow"
            && let Some(id) = block["id"]
                .as_str()
                .filter(|id| super::super::valid_component(id))
        {
            declarations.insert(id);
        }
    }
    let mut links = BTreeMap::new();
    for block in blocks() {
        let Some(call) = block["tool_use_id"]
            .as_str()
            .filter(|call| declarations.contains(call))
        else {
            continue;
        };
        if block["type"] != "tool_result" {
            continue;
        }
        let content = if let Some(text) = block["content"].as_str() {
            text.to_owned()
        } else {
            block["content"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|block| block["text"].as_str())
                .collect::<Vec<_>>()
                .join("\n")
        };
        if let Some(run) = content
            .lines()
            .find_map(|line| line.trim().strip_prefix("Run ID:").map(str::trim))
            .filter(|run| run.starts_with("wf_") && super::super::valid_component(run))
        {
            links.insert(run.to_owned(), call.to_owned());
        }
    }
    links
}

fn summary(path: &Path) -> Option<Value> {
    let metadata = path.symlink_metadata().ok()?;
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > 1024 * 1024 {
        return None;
    }
    if path
        .parent()?
        .symlink_metadata()
        .ok()?
        .file_type()
        .is_symlink()
    {
        return None;
    }
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .ok()?
        .take(1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    (bytes.len() <= 1024 * 1024)
        .then(|| serde_json::from_slice(&bytes).ok())
        .flatten()
}

fn project(
    client: &ClaudeClient,
    binding: (&Path, &str, &str),
    parent: &SessionHistory,
    summary: &Value,
) -> Result<(NativeSubagent, SessionHistory), AgentSessionError> {
    let (directory, run, call) = binding;
    let cwd = &parent.descriptor.cwd;
    let root = &parent.descriptor.provider_handle_id;
    let start = summary["startTime"]
        .as_i64()
        .and_then(chrono::DateTime::from_timestamp_millis)
        .map_or_else(|| parent.created_at.clone(), |time| time.to_rfc3339());
    let finish = summary["timestamp"]
        .as_str()
        .filter(|time| chrono::DateTime::parse_from_rfc3339(time).is_ok())
        .unwrap_or(&start);
    let description = summary["summary"]
        .as_str()
        .or_else(|| summary["workflowName"].as_str());
    let status = match summary["status"].as_str() {
        Some("completed") => "completed",
        Some("canceled" | "cancelled" | "killed" | "stopped") => "canceled",
        _ => "failed",
    };
    let entries = entries(client, (directory, run), summary, (&start, finish))?;
    let descriptor = json!({"id":call,"provider":"claude","title":"Workflow","description":description,"status":status,"createdAt":start,"updatedAt":finish,"toolCallId":call,"cwd":cwd,"subtitle":summary["defaultModel"]});
    let handle = AgentPersistenceHandle {
        provider: "claude".to_owned(),
        session_id: call.to_owned(),
        native_handle: None,
        metadata: Some(BTreeMap::from([(
            super::super::LOCATOR.to_owned(),
            json!({"root":root,"cwd":cwd,"workflowRun":run}),
        )])),
    };
    Ok((
        NativeSubagent {
            id: call.to_owned(),
            parent_id: root.clone(),
            cwd: cwd.clone(),
            descriptor,
            persistence: Some(handle),
        },
        SessionHistory {
            descriptor: SessionDescriptor {
                provider_id: "claude".to_owned(),
                provider_label: "Claude Code".to_owned(),
                provider_handle_id: call.to_owned(),
                cwd: cwd.clone(),
                title: Some("Workflow".to_owned()),
                first_prompt_preview: description.map(str::to_owned),
                last_prompt_preview: description.map(str::to_owned),
                last_activity_at: finish.to_owned(),
            },
            created_at: start,
            config: StoredAgentConfig {
                model: summary["defaultModel"].as_str().map(str::to_owned),
                ..StoredAgentConfig::default()
            },
            active: false,
            parent_id: Some(root.clone()),
            entries,
            resume_metadata: BTreeMap::new(),
        },
    ))
}

fn child_records(directory: &Path) -> Result<Vec<Value>, AgentSessionError> {
    let mut records = Vec::new();
    let mut pending = vec![directory.to_path_buf()];
    let mut count = 0;
    while let Some(directory) = pending.pop() {
        for file in child_files(&directory)? {
            count += 1;
            if count > 256 {
                return Err(AgentSessionError::Failed);
            }
            let kind = file.file_type().map_err(|_| AgentSessionError::Failed)?;
            if kind.is_dir() {
                pending.push(file.path());
            } else if kind.is_file()
                && file
                    .path()
                    .extension()
                    .is_some_and(|extension| extension == "jsonl")
            {
                records.extend(
                    history::read_active_records(&file.path())
                        .map_err(|_| AgentSessionError::Failed)?,
                );
                if records.len() > 16384 {
                    return Err(AgentSessionError::Failed);
                }
            }
        }
    }
    records.sort_by(|left, right| left["timestamp"].as_str().cmp(&right["timestamp"].as_str()));
    Ok(records)
}

fn child_files(directory: &Path) -> Result<Vec<std::fs::DirEntry>, AgentSessionError> {
    let files = match std::fs::read_dir(directory) {
        Ok(files) => files,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Err(_) => return Err(AgentSessionError::Failed),
    };
    if directory
        .symlink_metadata()
        .map_err(|_| AgentSessionError::Failed)?
        .file_type()
        .is_symlink()
        || directory.parent().is_some_and(|parent| {
            parent
                .symlink_metadata()
                .is_ok_and(|metadata| metadata.file_type().is_symlink())
        })
    {
        return Err(AgentSessionError::Rejected);
    }
    let mut entries = Vec::new();
    for (index, file) in files.enumerate() {
        if index >= 256 {
            return Err(AgentSessionError::Failed);
        }
        entries.push(file.map_err(|_| AgentSessionError::Failed)?);
    }
    Ok(entries)
}

fn entries(
    client: &ClaudeClient,
    binding: (&Path, &str),
    summary: &Value,
    timestamps: (&str, &str),
) -> Result<Vec<crate::protocol::timeline::NativeItem>, AgentSessionError> {
    let (directory, run) = binding;
    let (start, finish) = timestamps;
    let description = summary["summary"]
        .as_str()
        .or_else(|| summary["workflowName"].as_str());
    let mut stream = streaming::Stream::new(client.images.clone());
    let mut entries = Vec::new();
    if let Some(text) = description {
        entries.push(streaming::entry(
            &format!("workflow:{run}:prompt"),
            json!({"type":"user_message","text":text}),
            &json!({"timestamp":start}),
        ));
    }
    for mut record in child_records(&directory.join("subagents").join("workflows").join(run))? {
        record["isSidechain"] = json!(false);
        record["parent_tool_use_id"] = Value::Null;
        if record["type"] == "assistant" && record["message"]["id"].is_null() {
            record["message"]["id"] = record["uuid"].clone();
        }
        if record["timestamp"].is_null() {
            record["timestamp"] = json!(start);
        }
        if record["type"] == "user"
            && !record["message"]["content"]
                .as_array()
                .is_some_and(|blocks| blocks.iter().any(|block| block["type"] == "tool_result"))
        {
            continue;
        }
        stream.record(&record)?;
        while let Some(event) = stream.events.pop_front() {
            if let AgentTurnEvent::Timeline(entry) = event {
                entries.push(entry);
            }
        }
    }
    if let Some(text) = super::format(&summary["result"])
        && !entries.iter().any(|entry| {
            entry.item["type"] == "assistant_message"
                && entry.item["text"]
                    .as_str()
                    .is_some_and(|old| old.trim() == text.trim())
        })
    {
        entries.push(streaming::entry(
            &format!("workflow:{run}:result"),
            json!({"type":"assistant_message","text":text}),
            &json!({"timestamp":finish}),
        ));
    }
    Ok(entries)
}
