//! Record codec and compatibility hydration, independent from use-case reducers.
use crate::control::catalog::builtin_providers;
use crate::control::catalog::migration::migrate_state;
use crate::control::errors::{error, serialization_error};
use crate::control::persistence::transaction::{RecordContext, RecordTransaction, TypedChange};
use crate::control::project::worktrees::session_worktree_path;
use crate::control::settings::{DEFAULT_AGENT_SETTING_ID, SMALL_AGENT_SETTING_ID};
use ait_contracts::{ApiError, ProjectView, SessionView, default_settings};
use ait_domain::ErrorCode;
use ait_ports::{ControlChange, ControlRead, ControlRecord, ControlRecordKind, ControlStoreError};
use serde_json::{Value, json};
use std::collections::BTreeMap;
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

pub(in crate::control) fn decode_records<C: RecordContext>(
    read: &ControlRead,
) -> Result<RecordTransaction<C>, ApiError> {
    let mut value = json!({});
    for (field, kind) in C::FIELDS {
        value[*field] = match kind {
            ControlRecordKind::ProviderCredential
            | ControlRecordKind::RunCredential
            | ControlRecordKind::WorkspaceRunJournal => json!({}),
            ControlRecordKind::Settings => {
                serde_json::to_value(default_settings()).map_err(serialization_error)?
            }
            _ => json!([]),
        };
    }
    value["settings_revision"] = json!(1);
    let legacy_run = read.records.iter().any(|record| {
        record.kind == ControlRecordKind::Run && record.value.get("config").is_none()
    });
    for record in &read.records {
        let field = C::FIELDS
            .iter()
            .find(|(_, kind)| *kind == record.kind)
            .map(|(field, _)| *field);
        // Legacy Run migration can read its Agent as a codec-only dependency.
        let field = field.or_else(|| {
            (record.kind == ControlRecordKind::Agent && legacy_run).then_some("agents")
        });
        let field = field.ok_or_else(|| {
            error(
                ErrorCode::RunRecoveryFailed,
                "record read includes an undeclared context family",
                false,
            )
        })?;
        match record.kind {
            ControlRecordKind::ProviderCredential
            | ControlRecordKind::RunCredential
            | ControlRecordKind::WorkspaceRunJournal => {
                value[field][&record.id] = record.value.clone();
            }
            ControlRecordKind::Settings => {
                value["settings"] = record.value["values"].clone();
                value["settings_revision"] = record.value["revision"].clone();
            }
            _ => {
                if record.value.get("id").and_then(Value::as_str) != Some(record.id.as_str()) {
                    return Err(error(
                        ErrorCode::RunRecoveryFailed,
                        "control record identity does not match its payload",
                        false,
                    ));
                }
                if value.get(field).is_none() {
                    value[field] = json!([]);
                }
                value[field]
                    .as_array_mut()
                    .expect("record array")
                    .push(record.value.clone());
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
    } else if value.get("providers").is_none() {
        value["providers"] = json!([]);
    }
    value = migrate_state(value)?;
    if C::FIELDS
        .iter()
        .any(|(_, kind)| *kind == ControlRecordKind::Provider)
    {
        let providers = value["providers"].as_array_mut().expect("providers array");
        for provider in builtin_providers() {
            if !providers.iter().any(|p| p["id"] == provider.provider.id) {
                providers.push(serde_json::to_value(provider).map_err(serialization_error)?);
            }
        }
    }
    if let Some(settings) = value["settings"].as_object_mut() {
        settings.retain(|id, _| !id.starts_with("models."));
        let defaults = default_settings();
        for id in [DEFAULT_AGENT_SETTING_ID, SMALL_AGENT_SETTING_ID] {
            settings.entry(id).or_insert_with(|| defaults.0[id].clone());
        }
    }
    hydrate_session_workdirs(&mut value)?;
    let original = serde_json::from_value(value).map_err(serialization_error)?;
    Ok(RecordTransaction::new(
        read.revision,
        original,
        &read.records,
    ))
}

fn hydrate_session_workdirs(value: &mut Value) -> Result<(), ApiError> {
    let projects: Vec<ProjectView> =
        serde_json::from_value(value.get("projects").cloned().unwrap_or(json!([])))
            .map_err(serialization_error)?;
    if let Some(sessions) = value["sessions"].as_array_mut() {
        for raw in sessions {
            let mut session: SessionView =
                serde_json::from_value(raw.clone()).map_err(serialization_error)?;
            let Some(project) = projects
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
            *raw = serde_json::to_value(session).map_err(serialization_error)?;
        }
    }
    Ok(())
}

pub(in crate::control) fn encode_change(
    change: TypedChange,
    projects: &BTreeMap<(ControlRecordKind, String), Option<String>>,
) -> Result<ControlChange, ControlStoreError> {
    use ControlRecordKind as Kind;
    let (kind, id, project_id, value) = match change {
        TypedChange::Project(v) => (
            Kind::Project,
            v.id.clone(),
            Some(v.id.clone()),
            serde_json::to_value(v),
        ),
        TypedChange::Agent(v) => (Kind::Agent, v.id.clone(), None, serde_json::to_value(v)),
        TypedChange::Provider(v) => (
            Kind::Provider,
            v.provider.id.clone(),
            None,
            serde_json::to_value(v),
        ),
        TypedChange::Session(v) => (
            Kind::Session,
            v.id.clone(),
            Some(v.project_id.clone()),
            serde_json::to_value(v),
        ),
        TypedChange::Message(v) => (
            Kind::Message,
            v.id.clone(),
            Some(v.project_id.clone()),
            serde_json::to_value(v),
        ),
        TypedChange::Run(v) => (
            Kind::Run,
            v.id.clone(),
            Some(v.project_id.clone()),
            serde_json::to_value(v),
        ),
        TypedChange::Cron(v) => (
            Kind::Cron,
            v.id.clone(),
            Some(v.project_id.clone()),
            serde_json::to_value(v),
        ),

        TypedChange::ProviderCredential(id, v) => {
            (Kind::ProviderCredential, id, None, serde_json::to_value(v))
        }
        TypedChange::RunCredential(id, v) => (
            Kind::RunCredential,
            id.clone(),
            run_project(&id, projects)?,
            serde_json::to_value(v),
        ),
        TypedChange::WorkspaceRunJournal(id, v) => (
            Kind::WorkspaceRunJournal,
            id.clone(),
            run_project(&id, projects)?,
            serde_json::to_value(v),
        ),
        TypedChange::Settings(values, revision) => (
            Kind::Settings,
            "settings".into(),
            None,
            Ok(json!({"values": values, "revision": revision})),
        ),
        TypedChange::Delete(kind, id) => return Ok(ControlChange::Delete { kind, id }),
    };
    Ok(ControlChange::Put(ControlRecord {
        kind,
        id,
        project_id,
        value: value.map_err(|e| ControlStoreError::Other(e.to_string()))?,
    }))
}

fn run_project(
    id: &str,
    projects: &BTreeMap<(ControlRecordKind, String), Option<String>>,
) -> Result<Option<String>, ControlStoreError> {
    projects
        .get(&(ControlRecordKind::Run, id.to_owned()))
        .cloned()
        .filter(Option::is_some)
        .ok_or_else(|| {
            ControlStoreError::Other("Run-scoped change has no loaded or newly created Run".into())
        })
}
