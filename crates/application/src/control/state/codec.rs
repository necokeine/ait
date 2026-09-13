//! Record encoding, decoding, hydration and change calculation.
use crate::control::catalog::builtin_providers;
use crate::control::catalog::migration::migrate_state;
use crate::control::errors::{error, serialization_error};
use crate::control::project::worktrees::session_worktree_path;
use crate::control::state::{LoadedWorkingSet, WorkingSet, default_settings_revision};
use ait_contracts::{ApiError, default_settings};
use ait_domain::ErrorCode;
use ait_ports::{ControlChange, ControlRead, ControlRecord, ControlRecordKind};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap};
use std::path::Path;

pub(in crate::control) fn record_value<'a>(
    read: &'a ControlRead,
    kind: ControlRecordKind,
    id: &str,
) -> Option<&'a Value> {
    read.records
        .iter()
        .find(|record| record.kind == kind && record.id == id)
        .map(|record| &record.value)
}

pub(in crate::control) fn required_string(value: &Value, field: &str) -> Result<String, ApiError> {
    value
        .get(field)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| {
            error(
                ErrorCode::RunRecoveryFailed,
                format!("control record is missing {field}"),
                false,
            )
        })
}

pub(in crate::control) fn agent_provider_id(agent: &Value) -> Result<String, ApiError> {
    agent
        .pointer("/config/provider_id")
        .and_then(Value::as_str)
        .or_else(|| {
            agent
                .get("mode")
                .and_then(Value::as_str)
                .map(|kind| match kind {
                    "codex" => "builtin-codex",
                    "openai" => "builtin-openai",
                    "deepseek" => "builtin-deepseek",
                    _ => kind,
                })
        })
        .map(str::to_owned)
        .ok_or_else(|| {
            error(
                ErrorCode::InvalidAgentConfiguration,
                "Agent provider reference is missing",
                false,
            )
        })
}

pub(in crate::control) fn decode_records(read: ControlRead) -> Result<LoadedWorkingSet, ApiError> {
    let mut value = json!({
        "projects": [],
        "agents": [],
        "providers": [],
        "provider_credentials": {},
        "run_credentials": {},
        "sessions": [],
        "messages": [],
        "runs": [],
        "workspace_run_journals": {},
        "crons": [],
        "settings": default_settings(),
        "settings_revision": default_settings_revision(),
    });
    for record in read.records {
        match record.kind {
            ControlRecordKind::Project => value["projects"]
                .as_array_mut()
                .expect("record array")
                .push(record.value),
            ControlRecordKind::Agent => value["agents"]
                .as_array_mut()
                .expect("record array")
                .push(record.value),
            ControlRecordKind::Provider => value["providers"]
                .as_array_mut()
                .expect("record array")
                .push(record.value),
            ControlRecordKind::ProviderCredential => {
                value["provider_credentials"][record.id] = record.value;
            }
            ControlRecordKind::RunCredential => {
                value["run_credentials"][record.id] = record.value;
            }
            ControlRecordKind::Session => value["sessions"]
                .as_array_mut()
                .expect("record array")
                .push(record.value),
            ControlRecordKind::Message => value["messages"]
                .as_array_mut()
                .expect("record array")
                .push(record.value),
            ControlRecordKind::Run => value["runs"]
                .as_array_mut()
                .expect("record array")
                .push(record.value),
            ControlRecordKind::WorkspaceRunJournal => {
                value["workspace_run_journals"][record.id] = record.value;
            }
            ControlRecordKind::Cron => value["crons"]
                .as_array_mut()
                .expect("record array")
                .push(record.value),
            ControlRecordKind::Settings => {
                value["settings"] = record.value["values"].clone();
                value["settings_revision"] = record.value["revision"].clone();
            }
        }
    }
    if value["agents"]
        .as_array()
        .into_iter()
        .flatten()
        .any(|agent| agent.get("config").is_none())
    {
        value
            .as_object_mut()
            .expect("record object")
            .remove("providers");
    }
    value = migrate_state(value)?;
    let mut state: WorkingSet = serde_json::from_value(value).map_err(serialization_error)?;
    hydrate_session_workdirs(&mut state)?;
    for provider in builtin_providers() {
        if !state
            .providers
            .iter()
            .any(|existing| existing.provider.id == provider.provider.id)
        {
            state.providers.push(provider);
        }
    }
    state.settings.0.retain(|id, _| !id.starts_with("models."));
    Ok(LoadedWorkingSet {
        revision: read.revision,
        original: state,
    })
}

fn hydrate_session_workdirs(state: &mut WorkingSet) -> Result<(), ApiError> {
    for session in &mut state.sessions {
        let Some(project) = state
            .projects
            .iter()
            .find(|project| project.id == session.project_id)
        else {
            continue;
        };
        let expected = session_worktree_path(&project.workdir, &session.id)?;
        if session.workdir.is_empty() {
            session.workdir = expected.to_string_lossy().into_owned();
        } else if Path::new(&session.workdir) != expected {
            return Err(error(
                ErrorCode::InvalidSession,
                "Session workdir does not match its Project and id",
                false,
            ));
        }
    }
    Ok(())
}

pub(in crate::control) fn record_changes(
    original: &WorkingSet,
    updated: &WorkingSet,
) -> Result<Vec<ControlChange>, ApiError> {
    let original = encode_records(original)?;
    let updated = encode_records(updated)?;
    let mut changes = Vec::new();
    for (key, record) in &updated {
        if original.get(key) != Some(record) {
            changes.push(ControlChange::Put(record.clone()));
        }
    }
    for ((kind, id), _) in original {
        if !updated.contains_key(&(kind, id.clone())) {
            changes.push(ControlChange::Delete { kind, id });
        }
    }
    Ok(changes)
}

#[allow(clippy::too_many_lines)]
fn encode_records(
    state: &WorkingSet,
) -> Result<BTreeMap<(ControlRecordKind, String), ControlRecord>, ApiError> {
    let mut records = BTreeMap::new();
    let mut insert = |kind, id: String, project_id: Option<String>, value| {
        records.insert(
            (kind, id.clone()),
            ControlRecord {
                kind,
                id,
                project_id,
                value,
            },
        );
    };
    for project in &state.projects {
        insert(
            ControlRecordKind::Project,
            project.id.clone(),
            Some(project.id.clone()),
            serde_json::to_value(project).map_err(serialization_error)?,
        );
    }
    for agent in &state.agents {
        insert(
            ControlRecordKind::Agent,
            agent.id.clone(),
            None,
            serde_json::to_value(agent).map_err(serialization_error)?,
        );
    }
    for provider in &state.providers {
        insert(
            ControlRecordKind::Provider,
            provider.provider.id.clone(),
            None,
            serde_json::to_value(provider).map_err(serialization_error)?,
        );
    }
    for (id, reference) in &state.provider_credentials {
        insert(
            ControlRecordKind::ProviderCredential,
            id.clone(),
            None,
            json!(reference),
        );
    }
    let run_projects = state
        .runs
        .iter()
        .map(|run| (run.id.as_str(), run.project_id.as_str()))
        .collect::<HashMap<_, _>>();
    for (id, reference) in &state.run_credentials {
        insert(
            ControlRecordKind::RunCredential,
            id.clone(),
            run_projects.get(id.as_str()).map(|id| (*id).to_owned()),
            json!(reference),
        );
    }
    for session in &state.sessions {
        insert(
            ControlRecordKind::Session,
            session.id.clone(),
            Some(session.project_id.clone()),
            serde_json::to_value(session).map_err(serialization_error)?,
        );
    }
    for message in &state.messages {
        insert(
            ControlRecordKind::Message,
            message.id.clone(),
            Some(message.project_id.clone()),
            serde_json::to_value(message).map_err(serialization_error)?,
        );
    }
    for run in &state.runs {
        insert(
            ControlRecordKind::Run,
            run.id.clone(),
            Some(run.project_id.clone()),
            serde_json::to_value(run).map_err(serialization_error)?,
        );
    }
    for (id, journal) in &state.workspace_run_journals {
        insert(
            ControlRecordKind::WorkspaceRunJournal,
            id.clone(),
            run_projects.get(id.as_str()).map(|id| (*id).to_owned()),
            serde_json::to_value(journal).map_err(serialization_error)?,
        );
    }
    for cron in &state.crons {
        insert(
            ControlRecordKind::Cron,
            cron.id.clone(),
            Some(cron.project_id.clone()),
            serde_json::to_value(cron).map_err(serialization_error)?,
        );
    }
    insert(
        ControlRecordKind::Settings,
        "settings".into(),
        None,
        json!({"values": state.settings, "revision": state.settings_revision}),
    );
    Ok(records)
}
