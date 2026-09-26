use std::fs::File;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};

use serde_json::Value;
use server_domain::agent_runtime::{AgentPersistenceHandle, StoredAgentConfig};
use uuid::Uuid;

use super::{ClaudeClient, streaming::Stream};
use crate::ports::agent_session::{AgentSessionError, AgentTurnEvent};
use crate::ports::native_history::{ListOptions, SessionDescriptor, SessionHistory};

const MAX_HISTORY: u64 = 32 * 1024 * 1024;

pub(super) fn validate_handle(handle: &AgentPersistenceHandle) -> Result<(), AgentSessionError> {
    if handle.provider != "claude" || Uuid::parse_str(&handle.session_id).is_err() {
        return Err(AgentSessionError::Rejected);
    }
    Ok(())
}

fn root(client: &ClaudeClient) -> Result<PathBuf, AgentSessionError> {
    client
        .config_dir
        .clone()
        .or_else(|| {
            std::env::var_os("HOME")
                .or_else(|| std::env::var_os("USERPROFILE"))
                .map(|home| PathBuf::from(home).join(".claude"))
        })
        .map(|root| root.join("projects"))
        .ok_or(AgentSessionError::Unavailable)
}

pub(super) fn project_dir(client: &ClaudeClient, cwd: &str) -> Result<PathBuf, AgentSessionError> {
    let path = Path::new(cwd)
        .canonicalize()
        .map_err(|_| AgentSessionError::Unavailable)?;
    let path = path.to_str().ok_or(AgentSessionError::Unavailable)?;
    #[cfg(target_os = "macos")]
    let normalized = {
        use unicode_normalization::UnicodeNormalization;
        path.nfc().collect::<String>()
    };
    #[cfg(target_os = "macos")]
    let path = normalized.as_str();
    // Match the SDK's JavaScript UTF-16 replace, 200-character cap and signed hash.
    let mut encoded = String::new();
    let mut hash = 0_i32;
    let mut length = 0;
    for unit in path.encode_utf16() {
        hash = hash.wrapping_mul(31).wrapping_add(i32::from(unit));
        if length < 200 {
            encoded.push(
                char::from_u32(u32::from(unit))
                    .filter(char::is_ascii_alphanumeric)
                    .unwrap_or('-'),
            );
        }
        length += 1;
    }
    if length > 200 {
        encoded.push('-');
        encoded.push_str(&base36(hash.unsigned_abs()));
    }
    Ok(root(client)?.join(encoded))
}

fn base36(mut value: u32) -> String {
    let mut digits = Vec::new();
    loop {
        digits
            .push(char::from_digit(value % 36, 36).expect("a base-36 remainder is a valid digit"));
        value /= 36;
        if value == 0 {
            break;
        }
    }
    digits.into_iter().rev().collect()
}

pub(super) fn read(
    client: &ClaudeClient,
    handle: &AgentPersistenceHandle,
    cwd: &str,
) -> Result<Option<SessionHistory>, AgentSessionError> {
    validate_handle(handle)?;
    let path = project_dir(client, cwd)?.join(format!("{}.jsonl", handle.session_id));
    let records = match read_records(&path) {
        Ok(records) => records,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return super::rewind::fresh_history(handle, cwd);
        }
        Err(_) => return Err(AgentSessionError::Failed),
    };
    let mut history = parse(&records, &handle.session_id, &client.images)?;
    if Path::new(&history.descriptor.cwd).canonicalize().ok() != Path::new(cwd).canonicalize().ok()
    {
        return Err(AgentSessionError::Rejected);
    }
    let notes = crate::local::notes::Notes::restore(
        handle
            .metadata
            .as_ref()
            .and_then(|metadata| metadata.get("controlNotes")),
    )?;
    notes.history(&mut history.entries);
    let inputs = super::inputs::Inputs::restore(
        handle
            .metadata
            .as_ref()
            .and_then(|metadata| metadata.get("clientMessageIds")),
    )?;
    for entry in &mut history.entries {
        inputs.decorate(entry);
    }
    if let Some(saved) = inputs.saved() {
        history
            .resume_metadata
            .insert("clientMessageIds".to_owned(), saved);
    }
    if let Some(saved) = notes.saved() {
        history
            .resume_metadata
            .insert("controlNotes".to_owned(), saved);
    }
    Ok(Some(history))
}

pub(super) fn read_records(path: &Path) -> std::io::Result<Vec<Value>> {
    read_records_mode(path, false)
}

pub(super) fn read_active_records(path: &Path) -> std::io::Result<Vec<Value>> {
    read_records_mode(path, true)
}

fn read_records_mode(path: &Path, allow_partial_tail: bool) -> std::io::Result<Vec<Value>> {
    let metadata = path.symlink_metadata()?;
    if !metadata.is_file() || metadata.len() > MAX_HISTORY {
        return Err(std::io::Error::other("Invalid Claude history file"));
    }
    let mut input = BufReader::new(File::open(path)?.take(MAX_HISTORY + 1));
    let mut records = Vec::new();
    let mut bytes = Vec::new();
    let mut total_bytes = 0_u64;
    loop {
        bytes.clear();
        if (&mut input)
            .take(2 * 1024 * 1024)
            .read_until(b'\n', &mut bytes)?
            == 0
        {
            break;
        }
        total_bytes = total_bytes.saturating_add(bytes.len() as u64);
        if total_bytes > MAX_HISTORY {
            return Err(std::io::Error::other("Claude history exceeds limit"));
        }
        if bytes.last() != Some(&b'\n') {
            if allow_partial_tail && bytes.len() < 2 * 1024 * 1024 && input.fill_buf()?.is_empty() {
                break;
            }
            return Err(std::io::Error::other("Incomplete Claude history"));
        }
        if bytes.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        records.push(serde_json::from_slice(&bytes)?);
        if records.len() > 16384 {
            return Err(std::io::Error::other("Claude history exceeds limit"));
        }
    }
    Ok(records)
}

pub(super) fn parse(
    records: &[Value],
    id: &str,
    images: &crate::local::images::ImageStore,
) -> Result<SessionHistory, AgentSessionError> {
    let mut stream = Stream::new(images.clone());
    let mut usage = crate::local::usage::ClaudeUsage::default();
    let mut entries = Vec::new();
    let mut cwd = None;
    let mut created = None;
    let mut updated = None;
    let mut title = None;
    let mut model = None;
    let mut active = false;
    for record in records {
        if record["isSidechain"] == true {
            continue;
        }
        if record
            .get("sessionId")
            .and_then(Value::as_str)
            .is_some_and(|session| session != id)
        {
            return Err(AgentSessionError::Failed);
        }
        if let Some(path) = record["cwd"].as_str() {
            cwd = Some(path.to_owned());
        }
        if let Some(timestamp) = record["timestamp"].as_str() {
            chrono::DateTime::parse_from_rfc3339(timestamp)
                .map_err(|_| AgentSessionError::Failed)?;
            created.get_or_insert_with(|| timestamp.to_owned());
            updated = Some(timestamp.to_owned());
        }
        if let Some(value) = record["customTitle"]
            .as_str()
            .or_else(|| record["summary"].as_str())
        {
            title = Some(value.to_owned());
        }
        if record["type"] == "user" && record["isMeta"] != true {
            active = true;
        }
        if record["type"] == "assistant" {
            if let Some(value) = record["message"]["model"]
                .as_str()
                .filter(|model| *model != "<synthetic>")
            {
                model = Some(value.to_owned());
            }
            active = record["message"]["stop_reason"] == "tool_use";
        }
        stream.record(record)?;
        usage.observe(record);
        if record["type"] == "result" {
            active = false;
            stream.result_text(record, id)?;
        }
        while let Some(event) = stream.events.pop_front() {
            if let AgentTurnEvent::Timeline(entry) = event {
                entries.push(entry);
            }
        }
    }
    let prompts: Vec<_> = entries
        .iter()
        .filter(|entry| entry.item["type"] == "user_message")
        .filter_map(|entry| entry.item["text"].as_str())
        .collect();
    let preview = |text: &&str| text.chars().take(300).collect::<String>();
    Ok(SessionHistory {
        resume_metadata: usage
            .saved()
            .map(|saved| std::collections::BTreeMap::from([("lastUsage".to_owned(), saved)]))
            .unwrap_or_default(),
        parent_id: None,
        created_at: created.ok_or(AgentSessionError::Failed)?,
        active: active || !stream.tools.is_empty(),
        config: StoredAgentConfig {
            model,
            ..StoredAgentConfig::default()
        },
        descriptor: SessionDescriptor {
            provider_id: "claude".to_owned(),
            provider_label: "Claude Code".to_owned(),
            provider_handle_id: id.to_owned(),
            cwd: cwd.ok_or(AgentSessionError::Failed)?,
            title,
            first_prompt_preview: prompts.first().map(preview),
            last_prompt_preview: prompts.last().map(preview),
            last_activity_at: updated.ok_or(AgentSessionError::Failed)?,
        },
        entries,
    })
}

pub(super) fn list(
    client: &ClaudeClient,
    options: &ListOptions,
) -> Result<Vec<SessionDescriptor>, AgentSessionError> {
    if !(1..=4096).contains(&options.scan_limit) {
        return Err(AgentSessionError::Rejected);
    }
    let directories = if let Some(cwd) = &options.cwd {
        vec![project_dir(client, cwd)?]
    } else {
        directories(&root(client)?)?
    };
    let mut candidates = Vec::new();
    for directory in directories {
        let files = match std::fs::read_dir(directory) {
            Ok(files) => files,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => return Err(AgentSessionError::Failed),
        };
        for file in files.take(16384) {
            let file = file.map_err(|_| AgentSessionError::Failed)?;
            let path = file.path();
            if path
                .extension()
                .is_none_or(|extension| extension != "jsonl")
            {
                continue;
            }
            let Some(id) = path.file_stem().and_then(|id| id.to_str()) else {
                continue;
            };
            if Uuid::parse_str(id).is_err() || !file.file_type().is_ok_and(|kind| kind.is_file()) {
                continue;
            }
            let modified = file
                .metadata()
                .and_then(|metadata| metadata.modified())
                .map_err(|_| AgentSessionError::Failed)?;
            candidates.push((modified, path));
            if candidates.len() > 16384 {
                return Err(AgentSessionError::Failed);
            }
        }
    }
    candidates
        .sort_unstable_by(|left, right| right.0.cmp(&left.0).then_with(|| left.1.cmp(&right.1)));
    let mut result = Vec::new();
    for (_, path) in candidates.into_iter().take(options.scan_limit) {
        let Some(id) = path.file_stem().and_then(|id| id.to_str()) else {
            continue;
        };
        if let Ok(records) = read_records(&path)
            && let Ok(history) = parse(&records, id, &client.images)
            && options.cwd.as_ref().is_none_or(|cwd| {
                Path::new(cwd).canonicalize().ok()
                    == Path::new(&history.descriptor.cwd).canonicalize().ok()
            })
        {
            result.push(history.descriptor);
        }
    }
    Ok(result)
}

fn directories(root: &Path) -> Result<Vec<PathBuf>, AgentSessionError> {
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Err(_) => return Err(AgentSessionError::Failed),
    };
    entries
        .take(4096)
        .filter_map(|entry| match entry {
            Ok(entry) if entry.file_type().is_ok_and(|kind| kind.is_dir()) => {
                Some(Ok(entry.path()))
            }
            Ok(_) => None,
            Err(_) => Some(Err(AgentSessionError::Failed)),
        })
        .collect()
}
