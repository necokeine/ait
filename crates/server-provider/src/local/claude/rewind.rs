//! Claude SDK-compatible fork copies and native tracked-file checkpoint restoration.

use std::collections::BTreeMap;
use std::io::Write;

use serde_json::{Value, json};
use server_domain::agent_runtime::{AgentPersistenceHandle, StoredAgentConfig};
use uuid::Uuid;

use super::{ClaudeClient, config, history, transport::Transport};
use crate::ports::agent_session::{AgentSessionError, AgentSessionSpec};
use crate::ports::native_history::{SessionDescriptor, SessionHistory};

pub(super) const FRESH: &str = "aitFreshSession";

pub(super) fn conversation(
    client: &ClaudeClient,
    handle: &AgentPersistenceHandle,
    spec: &AgentSessionSpec,
    message: &str,
) -> Result<SessionHistory, AgentSessionError> {
    let (source, records, target) = target(client, handle, spec, message)?;
    let id = Uuid::new_v4().to_string();
    let previous = records[..target]
        .iter()
        .rposition(|record| record["type"] == "assistant" && record["isSidechain"] != true);
    let Some(previous) = previous else {
        if source
            .entries
            .iter()
            .take_while(|entry| entry.item["messageId"] != message)
            .any(|entry| entry.item["type"] == "user_message")
        {
            return Err(AgentSessionError::Rejected);
        }
        return Ok(fresh(&id, spec));
    };
    let records = fork_records(&records[..=previous], &handle.session_id, &id)?;
    let mut fork = history::parse(&records, &id, &client.images)?;
    if fork.active {
        return Err(AgentSessionError::Rejected);
    }
    let directory = history::project_dir(client, &spec.cwd)?;
    let mut file =
        tempfile::NamedTempFile::new_in(&directory).map_err(|_| AgentSessionError::Failed)?;
    for record in &records {
        serde_json::to_writer(&mut file, record).map_err(|_| AgentSessionError::Failed)?;
        file.write_all(b"\n")
            .map_err(|_| AgentSessionError::Failed)?;
    }
    file.as_file()
        .sync_all()
        .map_err(|_| AgentSessionError::Failed)?;
    file.persist_noclobber(directory.join(format!("{id}.jsonl")))
        .map_err(|_| AgentSessionError::Failed)?;
    fork.config = spec.config.clone();
    Ok(fork)
}

fn target(
    client: &ClaudeClient,
    handle: &AgentPersistenceHandle,
    spec: &AgentSessionSpec,
    message: &str,
) -> Result<(SessionHistory, Vec<Value>, usize), AgentSessionError> {
    config::validate_spec(spec)?;
    history::validate_handle(handle)?;
    if Uuid::parse_str(message).is_err() {
        return Err(AgentSessionError::Rejected);
    }
    let source = history::read(client, handle, &spec.cwd)?.ok_or(AgentSessionError::Rejected)?;
    if !source
        .entries
        .iter()
        .any(|entry| entry.item["type"] == "user_message" && entry.item["messageId"] == message)
    {
        return Err(AgentSessionError::Rejected);
    }
    let records = history::read_records(
        &history::project_dir(client, &spec.cwd)?.join(format!("{}.jsonl", handle.session_id)),
    )
    .map_err(|_| AgentSessionError::Failed)?;
    let candidates: Vec<_> = records
        .iter()
        .enumerate()
        .filter(|(_, record)| {
            record["uuid"] == message && record["type"] == "user" && record["isSidechain"] != true
        })
        .map(|(index, _)| index)
        .collect();
    if candidates.len() != 1 {
        return Err(AgentSessionError::Rejected);
    }
    Ok((source, records, candidates[0]))
}

pub(super) async fn files(
    client: &ClaudeClient,
    handle: &AgentPersistenceHandle,
    spec: &AgentSessionSpec,
    message: &str,
) -> Result<(), AgentSessionError> {
    target(client, handle, spec, message)?;
    let mut transport = Transport::spawn(client, spec, Some((&handle.session_id, true)), None)?;
    let result = async {
        transport.initialize().await?;
        let result = transport
            .request(json!({"subtype":"rewind_files","user_message_id":message,"dry_run":false}))
            .await?;
        if result["canRewind"] == false
            || result["can_rewind"] == false
            || result["error"]
                .as_str()
                .is_some_and(|error| !error.is_empty())
        {
            return Err(AgentSessionError::Rejected);
        }
        // The Python SDK returns an empty successful control response; TypeScript also reports
        // canRewind and a diff summary. Neither form is a model turn or a conversation mutation.
        Ok(())
    }
    .await;
    transport.close().await?;
    result
}

fn fork_records(
    records: &[Value],
    source: &str,
    id: &str,
) -> Result<Vec<Value>, AgentSessionError> {
    let mut parents: BTreeMap<String, Option<String>> = BTreeMap::new();
    let mut output = Vec::new();
    for record in records
        .iter()
        .filter(|record| record["isSidechain"] != true)
    {
        if !matches!(
            record["type"].as_str(),
            Some(
                "user" | "assistant" | "attachment" | "system" | "progress" | "content-replacement"
            )
        ) {
            continue;
        }
        let Some(uuid) = record["uuid"].as_str() else {
            continue;
        };
        if parents.contains_key(uuid) {
            return Err(AgentSessionError::Failed);
        }
        let parent = record["parentUuid"]
            .as_str()
            .and_then(|parent| parents.get(parent))
            .cloned()
            .flatten();
        if record["type"] == "progress" {
            parents.insert(uuid.to_owned(), parent);
            continue;
        }
        let new_uuid = Uuid::new_v4().to_string();
        let mut copied = record.clone();
        copied["uuid"] = json!(new_uuid);
        copied["sessionId"] = json!(id);
        if copied.get("session_id").is_some() {
            copied["session_id"] = json!(id);
        }
        copied["parentUuid"] = json!(parent);
        copied["logicalParentUuid"] = json!(
            record["logicalParentUuid"]
                .as_str()
                .and_then(|parent| parents.get(parent))
                .cloned()
                .flatten()
        );
        copied["isSidechain"] = json!(false);
        copied["forkedFrom"] = json!({"sessionId":source,"messageUuid":uuid});
        let object = copied.as_object_mut().ok_or(AgentSessionError::Failed)?;
        for key in ["teamName", "agentName", "slug", "sourceToolAssistantUUID"] {
            object.remove(key);
        }
        parents.insert(uuid.to_owned(), Some(new_uuid));
        output.push(copied);
    }
    if let Some(last) = output.last_mut() {
        last["timestamp"] = json!(chrono::Utc::now().to_rfc3339());
    }
    Ok(output)
}

pub(super) fn fresh(id: &str, spec: &AgentSessionSpec) -> SessionHistory {
    let now = chrono::Utc::now().to_rfc3339();
    SessionHistory {
        resume_metadata: BTreeMap::from([(
            FRESH.to_owned(),
            json!({"cwd":spec.cwd,"createdAt":now}),
        )]),
        parent_id: None,
        created_at: now.clone(),
        config: spec.config.clone(),
        active: false,
        entries: vec![],
        descriptor: SessionDescriptor {
            provider_id: "claude".into(),
            provider_label: "Claude Code".into(),
            provider_handle_id: id.to_owned(),
            cwd: spec.cwd.clone(),
            title: None,
            first_prompt_preview: None,
            last_prompt_preview: None,
            last_activity_at: now,
        },
    }
}

pub(super) fn fresh_history(
    handle: &AgentPersistenceHandle,
    cwd: &str,
) -> Result<Option<SessionHistory>, AgentSessionError> {
    let Some(marker) = handle
        .metadata
        .as_ref()
        .and_then(|metadata| metadata.get(FRESH))
    else {
        return Ok(None);
    };
    let stored_cwd = marker["cwd"].as_str().ok_or(AgentSessionError::Failed)?;
    let created = marker["createdAt"]
        .as_str()
        .ok_or(AgentSessionError::Failed)?;
    chrono::DateTime::parse_from_rfc3339(created).map_err(|_| AgentSessionError::Failed)?;
    if std::fs::canonicalize(cwd).ok() != std::fs::canonicalize(stored_cwd).ok() {
        return Err(AgentSessionError::Rejected);
    }
    let mut history = fresh(
        &handle.session_id,
        &AgentSessionSpec {
            provider: "claude".into(),
            cwd: cwd.to_owned(),
            config: StoredAgentConfig::default(),
        },
    );
    created.clone_into(&mut history.created_at);
    created.clone_into(&mut history.descriptor.last_activity_at);
    history
        .resume_metadata
        .insert(FRESH.to_owned(), marker.clone());
    Ok(Some(history))
}

#[cfg(test)]
mod tests;
