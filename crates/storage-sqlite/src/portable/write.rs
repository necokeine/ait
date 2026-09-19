//! One authoritative SQLite transaction per commit scope.
use sha2::{Digest, Sha256};

use super::{
    BTreeMap, BTreeSet, ControlChange, ControlStoreError, ControlVersion, Kind, OptionalExtension,
    PendingEvent, State, Value, apply_change, catalog_id, json_error, other, params, project,
    projection, read, revision, sql_error, table,
};

#[allow(
    clippy::too_many_lines,
    reason = "validate all scopes and dependencies before choosing the single authoritative transaction"
)]
pub(super) fn commit(
    state: &mut State,
    runtime: &str,
    expected: &ControlVersion,
    changes: Vec<ControlChange>,
    events: &[PendingEvent],
) -> Result<u64, ControlStoreError> {
    if expected.catalog_id != catalog_id(&state.catalog)? {
        return Err(ControlStoreError::Conflict);
    }
    for (id, version) in &expected.projects {
        let owned = state.projects.get(id).ok_or(ControlStoreError::Conflict)?;
        project::validate_owned(owned)?;
        project::check_stream(&state.catalog, &owned.connection, id)?;
        super::workers::assert_prior_quiescent(&owned.connection)?;
        if project::version(&owned.connection)?.owner(id) != version.owner(id) {
            return Err(ControlStoreError::Conflict);
        }
    }
    for change in &changes {
        if let ControlChange::Put(record) = change
            && record.kind == Kind::Run
            && record
                .project_id
                .as_ref()
                .is_some_and(|id| state.draining.contains(id))
            && read::location(state, Kind::Run, &record.id)?.is_none()
        {
            return Err(other("PROJECT_DRAINING: new Runs are not admitted"));
        }
    }
    let request = serde_json::to_string(&(expected, &changes, events)).map_err(json_error)?;
    let operation = format!("{:x}", Sha256::digest(request.as_bytes()));
    for id in expected.projects.keys() {
        let receipt: Option<u64> = state.projects[id]
            .connection
            .query_row(
                "SELECT revision FROM commit_receipts WHERE operation_id=?1 AND request_json=?2",
                params![operation, request],
                |row| row.get(0),
            )
            .optional()
            .map_err(sql_error)?;
        if let Some(result) = receipt {
            return Ok(result);
        }
    }
    if expected.observes_catalog && expected.catalog_revision != revision(&state.catalog)? {
        return Err(ControlStoreError::Conflict);
    }
    for (id, version) in &expected.projects {
        if &project::version(&state.projects[id].connection)? != version {
            return Err(ControlStoreError::Conflict);
        }
    }
    // Archive configuration is a Project snapshot. Import never installs presets or
    // credentials into the new host's catalog as a side effect of history creation.
    let registration = changes.iter().find_map(|change| match change {
        ControlChange::Put(record)
            if record.kind == Kind::Project && !state.projects.contains_key(&record.id) =>
        {
            Some(record.id.clone())
        }
        _ => None,
    });
    let sessions: BTreeMap<String, String> = changes
        .iter()
        .filter_map(|change| match change {
            ControlChange::Put(record) if record.kind == Kind::Session => record
                .project_id
                .as_ref()
                .map(|id| (record.id.clone(), id.clone())),
            _ => None,
        })
        .collect();
    let mut projects = BTreeSet::new();
    let mut has_global = false;
    for change in &changes {
        if let ControlChange::Put(record) = change {
            if let Some(existing) = read::location(state, record.kind, &record.id)?
                && let Some(destination) = destination(state, change, &sessions)?
                && existing != destination
            {
                return Err(other("A record cannot move between Projects"));
            }
            if let Some(id) = &registration
                && matches!(record.kind, Kind::Agent | Kind::Provider)
            {
                projects.insert(id.clone());
                continue;
            }
        }
        if let Some(id) = destination(state, change, &sessions)? {
            projects.insert(id);
        } else {
            has_global = true;
        }
    }
    if projects.len() > 1 || !projects.is_empty() && has_global {
        return Err(other(
            "A transaction must have exactly one authoritative scope",
        ));
    }
    if let Some(id) = projects.into_iter().next() {
        if !state.projects.contains_key(&id) {
            let registration = changes.iter().find_map(|change| match change {
                ControlChange::Put(record) if record.kind == Kind::Project && record.id == id => {
                    Some(record)
                }
                _ => None,
            });
            if let Some(record) = registration {
                let exists: bool = state
                    .catalog
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM projects WHERE id=?1)",
                        [&id],
                        |row| row.get(0),
                    )
                    .map_err(sql_error)?;
                if exists {
                    return Err(ControlStoreError::Conflict);
                }
                let root = record
                    .value
                    .get("workdir")
                    .and_then(Value::as_str)
                    .ok_or_else(|| other("Project workdir missing"))?;
                let root = std::path::PathBuf::from(root);
                let request =
                    serde_json::to_string(&(expected, &changes, events)).map_err(json_error)?;
                let next = project::create(state, &id, &root, runtime, |catalog, transaction| {
                    apply_project(catalog, transaction, &request, &changes, events)
                })?;
                let _projection =
                    projection::refresh(&mut state.catalog, &id, &state.projects[&id]);
                return Ok(next);
            }
            return Err(ControlStoreError::Conflict);
        }
        if !expected.projects.contains_key(&id) && revision(&state.projects[&id].connection)? != 0 {
            return Err(ControlStoreError::Conflict);
        }
        let next = project_commit(state, &id, expected, &changes, events)?;
        // A catalog projection failure cannot turn a committed Run into a failed input.
        let _projection = projection::refresh(&mut state.catalog, &id, &state.projects[&id]);
        Ok(next)
    } else {
        if expected.catalog_revision != revision(&state.catalog)? {
            return Err(ControlStoreError::Conflict);
        }
        let transaction = state.catalog.transaction().map_err(sql_error)?;
        for change in changes {
            apply_change(&transaction, change)?;
        }
        append_events(&transaction, events, true)?;
        transaction
            .execute(
                "UPDATE control_metadata SET revision=revision+1 WHERE singleton=1",
                [],
            )
            .map_err(sql_error)?;
        let next = revision(&transaction)?;
        transaction.commit().map_err(sql_error)?;
        Ok(next)
    }
}

fn destination(
    state: &State,
    change: &ControlChange,
    sessions: &BTreeMap<String, String>,
) -> Result<Option<String>, ControlStoreError> {
    match change {
        ControlChange::Put(record) => match record.kind {
            Kind::Project => Ok(Some(record.id.clone())),
            Kind::Message
            | Kind::Session
            | Kind::Run
            | Kind::RunCredential
            | Kind::WorkspaceRunJournal
            | Kind::Cron => record
                .project_id
                .clone()
                .map(Some)
                .ok_or_else(|| other("Project record has no Project identity")),
            Kind::Agent => {
                if let Some(session) = record.value.get("owner_session_id").and_then(Value::as_str)
                {
                    if let Some(id) = sessions.get(session) {
                        return Ok(Some(id.clone()));
                    }
                    return read::location(state, Kind::Session, session)?
                        .map(Some)
                        .ok_or_else(|| other("Private Agent has no owning Session"));
                }
                Ok(None)
            }
            Kind::Provider | Kind::ProviderCredential | Kind::Settings => Ok(None),
        },
        ControlChange::Delete { kind, id } => {
            if matches!(
                kind,
                Kind::Provider | Kind::ProviderCredential | Kind::Settings
            ) {
                return Ok(None);
            }
            read::location(state, *kind, id)
        }
    }
}

fn project_commit(
    state: &mut State,
    id: &str,
    expected: &ControlVersion,
    changes: &[ControlChange],
    events: &[PendingEvent],
) -> Result<u64, ControlStoreError> {
    let request = serde_json::to_string(&(expected, changes, events)).map_err(json_error)?;
    let operation = format!("{:x}", Sha256::digest(request.as_bytes()));
    let owned = state
        .projects
        .get_mut(id)
        .ok_or(ControlStoreError::Conflict)?;
    let transaction = owned.connection.transaction().map_err(sql_error)?;
    let duplicate: Option<u64> = transaction
        .query_row(
            "SELECT revision FROM commit_receipts WHERE operation_id=?1 AND request_json=?2",
            params![operation, request],
            |row| row.get(0),
        )
        .optional()
        .map_err(sql_error)?;
    if let Some(revision) = duplicate {
        return Ok(revision);
    }
    let next = apply_project(&state.catalog, &transaction, &request, changes, events)?;
    transaction.commit().map_err(sql_error)?;
    Ok(next)
}

fn apply_project(
    catalog: &rusqlite::Connection,
    transaction: &rusqlite::Transaction<'_>,
    request: &str,
    changes: &[ControlChange],
    events: &[PendingEvent],
) -> Result<u64, ControlStoreError> {
    let operation = format!("{:x}", Sha256::digest(request.as_bytes()));
    let next = revision(transaction)?
        .checked_add(1)
        .ok_or_else(|| other("Project revision exhausted"))?;
    super::native_bindings::reserve(transaction, changes)?;
    for change in changes {
        if let ControlChange::Put(record) = change {
            let project_id: String = transaction
                .query_row(
                    "SELECT project_id FROM project_identity WHERE singleton=1",
                    [],
                    |row| row.get(0),
                )
                .map_err(sql_error)?;
            if !matches!(record.kind, Kind::Agent | Kind::Provider)
                && record.project_id.as_deref() != Some(&project_id)
            {
                return Err(other(
                    "Project payload identity does not match its transaction",
                ));
            }
            if let Some(payload_id) = record.value.get("project_id").and_then(Value::as_str)
                && payload_id != project_id
            {
                return Err(other(
                    "Project payload identity does not match its record envelope",
                ));
            }
            if matches!(record.kind, Kind::Agent | Kind::Provider) {
                let source = if record.kind == Kind::Agent
                    && record
                        .value
                        .get("owner_session_id")
                        .is_some_and(|value| !value.is_null())
                {
                    catalog_id(catalog)?
                } else {
                    format!("archive:{operation}")
                };
                transaction
                    .execute(
                        "INSERT OR IGNORE INTO configuration_sources VALUES(?1,?2,?3)",
                        params![table(record.kind), record.id, source],
                    )
                    .map_err(sql_error)?;
            }
            if record.kind == Kind::Cron {
                transaction.execute("INSERT INTO configuration_sources VALUES('crons',?1,?2) ON CONFLICT(kind,id) DO UPDATE SET catalog_id=excluded.catalog_id",params![record.id,catalog_id(catalog)?]).map_err(sql_error)?;
            }
        }
        apply_change(transaction, change.clone())?;
        let (kind, record_id, deleted) = match change {
            ControlChange::Put(record) => (record.kind, record.id.as_str(), false),
            ControlChange::Delete { kind, id } => (*kind, id.as_str(), true),
        };
        transaction
            .execute(
                "INSERT INTO projection_changes VALUES(?1,?2,?3,?4)",
                params![next, table(kind), record_id, deleted],
            )
            .map_err(sql_error)?;
    }
    projection::snapshots(catalog, transaction)?;
    append_events(transaction, events, false)?;
    transaction
        .execute(
            "UPDATE control_metadata SET revision=?1 WHERE singleton=1",
            [next],
        )
        .map_err(sql_error)?;
    transaction
        .execute(
            "INSERT INTO commit_receipts VALUES(?1,?2,?3)",
            params![operation, request, next],
        )
        .map_err(sql_error)?;
    Ok(next)
}

pub(super) fn append_events(
    connection: &rusqlite::Connection,
    events: &[PendingEvent],
    global: bool,
) -> Result<(), ControlStoreError> {
    for event in events {
        let sql = if global {
            "INSERT INTO durable_events(kind,entity_id,body_json,created_at,project_id) VALUES(?1,?2,?3,?4,NULL)"
        } else {
            "INSERT INTO durable_events(kind,entity_id,body_json,created_at) VALUES(?1,?2,?3,?4)"
        };
        connection
            .execute(
                sql,
                params![
                    event.kind,
                    event.entity_id,
                    serde_json::to_string(&event.body).map_err(json_error)?,
                    event.created_at
                ],
            )
            .map_err(sql_error)?;
    }
    Ok(())
}
