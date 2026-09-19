//! Explicit, resumable conversion. The original catalog resolves legacy decisions first.
use super::{
    BTreeMap, Connection, ControlChange, ControlFilter, ControlStoreError, FORMAT, GLOBAL_ID, Kind,
    OptionalExtension, PROJECT_ID, Path, apply_change, catalog_lock, io_error, other, ownership,
    params, random_id, read_filter, revision, sql_error,
};

pub(super) fn upgrade(path: &Path) -> Result<(), ControlStoreError> {
    let path = std::fs::canonicalize(path).map_err(io_error)?;
    let _lock = catalog_lock(&path.with_file_name(format!(
            "{}.lock",
            path.file_name()
                .ok_or_else(|| other("catalog filename missing"))?
                .to_string_lossy()
        )))?;
    let mut catalog = Connection::open(&path).map_err(sql_error)?;
    catalog
        .execute_batch("PRAGMA foreign_keys=ON; PRAGMA synchronous=FULL;")
        .map_err(sql_error)?;
    let format: u32 = catalog
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(sql_error)?;
    let application: u32 = catalog
        .pragma_query_value(None, "application_id", |row| row.get(0))
        .map_err(sql_error)?;
    if application != GLOBAL_ID || !matches!(format, 2 | FORMAT) {
        return Err(other(
            "Only a completed format-2 catalog can be converted; earlier or unknown formats require their original upgrade tool",
        ));
    }
    let mut projects = BTreeMap::new();
    read_filter(&catalog, &ControlFilter::all(Kind::Project), &mut projects)?;
    // Retain every lock through recovery, backup and the format barrier. Old daemons
    // must already be stopped: their protocol did not retain these locks.
    let mut locks = Vec::new();
    for record in projects.values() {
        locks.push(ownership::acquire(root(record)?, &record.id)?);
    }
    if format == 2 {
        super::super::split::recover(&mut catalog)?;
        let mut recovered = BTreeMap::new();
        read_filter(&catalog, &ControlFilter::all(Kind::Project), &mut recovered)?;
        for (key, record) in &recovered {
            if !projects.contains_key(key) {
                locks.push(ownership::acquire(root(record)?, &record.id)?);
            }
        }
        projects = recovered;
        backup(&catalog, &path)?;
        for record in projects.values() {
            let source = Connection::open_with_flags(
                root(record)?.join(".ait/project.sqlite3"),
                rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE,
            )
            .map_err(sql_error)?;
            verify_source(&catalog, &source, &record.id)?;
            backup(&source, &root(record)?.join(".ait/project.sqlite3"))?;
        }
        let transaction = catalog.transaction().map_err(sql_error)?;
        transaction
            .execute_batch(include_str!("catalog.sql"))
            .map_err(sql_error)?;
        transaction
            .execute("DELETE FROM durable_events", [])
            .map_err(sql_error)?;
        transaction.execute("UPDATE portable_catalog SET catalog_id=(SELECT coordinator_id FROM split_metadata WHERE singleton=1)",[]).map_err(sql_error)?;
        for record in projects.values() {
            transaction
                .execute(
                    "INSERT INTO conversion_manifest VALUES(?1,?2,'preparing')",
                    params![record.id, random_id(&transaction)?],
                )
                .map_err(sql_error)?;
        }
        transaction
            .pragma_update(None, "user_version", FORMAT)
            .map_err(sql_error)?;
        transaction.commit().map_err(sql_error)?;
    }
    for record in projects.values() {
        let operation: Option<String> = catalog.query_row("SELECT operation_id FROM conversion_manifest WHERE project_id=?1 AND phase!='converted'",[&record.id],|row|row.get(0)).optional().map_err(sql_error)?;
        if let Some(operation) = operation {
            convert_project(&catalog, record, &operation)?;
            catalog
                .execute(
                    "UPDATE conversion_manifest SET phase='converted' WHERE project_id=?1",
                    [&record.id],
                )
                .map_err(sql_error)?;
        }
    }
    Ok(())
}

fn root(record: &super::ControlRecord) -> Result<&Path, ControlStoreError> {
    record
        .value
        .get("workdir")
        .and_then(super::Value::as_str)
        .map(Path::new)
        .ok_or_else(|| other("Project workdir missing"))
}

fn verify_source(
    catalog: &Connection,
    project: &Connection,
    id: &str,
) -> Result<(), ControlStoreError> {
    let format: u32 = project
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(sql_error)?;
    let application: u32 = project
        .pragma_query_value(None, "application_id", |row| row.get(0))
        .map_err(sql_error)?;
    if format != 2 || application != PROJECT_ID {
        return Err(other("Legacy Project format is not convertible"));
    }
    let (saved, owner): (String, String) = project
        .query_row(
            "SELECT project_id,coordinator_id FROM project_identity WHERE singleton=1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(sql_error)?;
    let expected: String = catalog
        .query_row(
            "SELECT coordinator_id FROM split_metadata WHERE singleton=1",
            [],
            |row| row.get(0),
        )
        .map_err(sql_error)?;
    if saved != id || owner != expected {
        return Err(other(
            "LEGACY_RECOVERY_REQUIRED: conversion requires this Project's original catalog",
        ));
    }
    Ok(())
}

#[allow(
    clippy::too_many_lines,
    reason = "one auditable conversion transaction preserves immutable records and its completion receipt"
)]
fn convert_project(
    catalog: &Connection,
    record: &super::ControlRecord,
    operation: &str,
) -> Result<(), ControlStoreError> {
    let path = root(record)?.join(".ait/project.sqlite3");
    let mut project =
        Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE)
            .map_err(sql_error)?;
    let format: u32 = project
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(sql_error)?;
    if format == FORMAT {
        let complete: bool = project
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM commit_receipts WHERE operation_id=?1)",
                [operation],
                |row| row.get(0),
            )
            .map_err(sql_error)?;
        return if complete {
            Ok(())
        } else {
            Err(other(
                "Project conversion receipt does not match the catalog manifest",
            ))
        };
    }
    verify_source(catalog, &project, &record.id)?;
    let transaction = project.transaction().map_err(sql_error)?;
    // Add new tables without rebuilding immutable Message rows or their foreign keys.
    let schema = include_str!("project.sql");
    for section in schema.split("CREATE TABLE ").skip(1) {
        let name = section
            .split_whitespace()
            .next()
            .ok_or_else(|| other("invalid schema"))?;
        if matches!(
            name,
            "control_metadata"
                | "projects"
                | "agents"
                | "agent_providers"
                | "configuration_sources"
                | "configuration_bindings"
                | "crons"
                | "commit_receipts"
                | "projection_changes"
                | "worker_processes"
                | "native_bindings"
        ) {
            transaction
                .execute_batch(&format!("CREATE TABLE {section}"))
                .map_err(sql_error)?;
        }
    }
    transaction.execute_batch("ALTER TABLE project_identity ADD COLUMN stream_id TEXT NOT NULL DEFAULT ''; ALTER TABLE project_identity ADD COLUMN owner_epoch INTEGER NOT NULL DEFAULT 0; ALTER TABLE project_identity ADD COLUMN owner_instance TEXT NOT NULL DEFAULT ''; ALTER TABLE project_identity ADD COLUMN origin_catalog TEXT NOT NULL DEFAULT ''; DELETE FROM prepared_commit;").map_err(sql_error)?;
    let source = super::catalog_id(catalog)?;
    transaction
        .execute(
            "UPDATE project_identity SET stream_id=?1,origin_catalog=?2 WHERE singleton=1",
            params![random_id(&transaction)?, source],
        )
        .map_err(sql_error)?;
    transaction
        .execute(
            "UPDATE control_metadata SET revision=?1 WHERE singleton=1",
            [revision(catalog)?.max(1)],
        )
        .map_err(sql_error)?;
    apply_change(&transaction, ControlChange::Put(record.clone()))?;
    let mut records = BTreeMap::new();
    read_filter(
        catalog,
        &ControlFilter::project(Kind::Cron, &record.id),
        &mut records,
    )?;
    let mut agents = BTreeMap::new();
    read_filter(catalog, &ControlFilter::all(Kind::Agent), &mut agents)?;
    for agent in agents.into_values() {
        let Some(session) = agent
            .value
            .get("owner_session_id")
            .and_then(super::Value::as_str)
        else {
            continue;
        };
        let exists: bool = transaction
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sessions WHERE id=?1)",
                [session],
                |row| row.get(0),
            )
            .map_err(sql_error)?;
        if exists {
            records.insert((Kind::Agent, agent.id.clone()), agent);
        }
    }
    for record in records.into_values() {
        apply_change(&transaction, ControlChange::Put(record))?;
    }
    super::projection::snapshots(catalog, &transaction)?;
    let mut sessions = BTreeMap::new();
    read_filter(
        &transaction,
        &ControlFilter::all(Kind::Session),
        &mut sessions,
    )?;
    super::native_bindings::reserve(
        &transaction,
        &sessions
            .into_values()
            .map(ControlChange::Put)
            .collect::<Vec<_>>(),
    )?;
    transaction
        .execute(
            "INSERT INTO commit_receipts VALUES(?1,'legacy-conversion',?2)",
            params![operation, revision(&transaction)?],
        )
        .map_err(sql_error)?;
    transaction
        .pragma_update(None, "user_version", FORMAT)
        .map_err(sql_error)?;
    transaction.commit().map_err(sql_error)
}

fn backup(connection: &Connection, path: &Path) -> Result<(), ControlStoreError> {
    let destination = path.with_file_name(format!(
        "{}.{}.pre-portable.sqlite3",
        path.file_name()
            .ok_or_else(|| other("backup filename missing"))?
            .to_string_lossy(),
        random_id(connection)?
    ));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options.open(&destination).map_err(io_error)?;
    connection
        .backup(super::super::MAIN_DB, &destination, None)
        .map_err(sql_error)?;
    file.sync_all().map_err(io_error)
}
