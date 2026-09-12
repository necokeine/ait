use super::{
    BTreeMap, Connection, ControlChange, ControlFilter, ControlRecordKind, ControlStoreError,
    GLOBAL_APPLICATION_ID, MAIN_DB, OptionalExtension, PROJECT_KINDS, Path, PathBuf, RECORD_SCHEMA,
    Transaction, Value, build_commit, ensure_target, io_error, migrate_legacy_blob, other,
    pragma_number, progress_rows, read_filter, recover, replay_locked, revision, route_event,
    sql_error, submit, table_count,
};

pub(super) fn initialize(
    connection: &mut Connection,
    path: &Path,
) -> Result<(), ControlStoreError> {
    let application = pragma_number(connection, "application_id")?;
    let version = pragma_number(connection, "user_version")?;
    if application != 0 && application != GLOBAL_APPLICATION_ID || version > 1 {
        return Err(other(format!(
            "unsupported global database format (application={application}, version={version})"
        )));
    }
    if has_table(connection, "split_metadata")? {
        connection
            .execute_batch("PRAGMA foreign_keys=ON; PRAGMA synchronous=FULL;")
            .map_err(sql_error)?;
        recover(connection)?;
        let migrated: bool = connection
            .query_row(
                "SELECT migrated FROM split_metadata WHERE singleton=1",
                [],
                |row| row.get(0),
            )
            .map_err(sql_error)?;
        if !migrated {
            migrate(connection)?;
        }
        if pragma_number(connection, "application_id")? != GLOBAL_APPLICATION_ID
            || pragma_number(connection, "user_version")? != 1
        {
            return Err(other("global database format marker is invalid"));
        }
        return Ok(());
    }
    let legacy =
        has_table(connection, "control_metadata")? || has_table(connection, "control_state")?;
    if legacy {
        backup_before_migration(connection, path)?;
        connection.execute_batch(RECORD_SCHEMA).map_err(sql_error)?;
        migrate_legacy_blob(connection)?;
        let has_route: bool = connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_table_info('durable_events') WHERE name='project_id')",
            [], |row| row.get(0),
        ).map_err(sql_error)?;
        if !has_route {
            connection
                .execute("ALTER TABLE durable_events ADD COLUMN project_id TEXT", [])
                .map_err(sql_error)?;
        }
    } else {
        if table_count(connection)? != 0 {
            return Err(other("database is not an AIT catalog"));
        }
        connection
            .execute_batch(
                "PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; PRAGMA foreign_keys=ON;",
            )
            .map_err(sql_error)?;
        let transaction = connection.transaction().map_err(sql_error)?;
        let schema = include_str!("global.sql")
            .splitn(4, ';')
            .nth(3)
            .expect("global schema");
        transaction.execute_batch(schema).map_err(sql_error)?;
        transaction
            .execute_batch(include_str!("coordinator.sql"))
            .map_err(sql_error)?;
        mark_format(&transaction)?;
        return transaction.commit().map_err(sql_error);
    }
    let transaction = connection.transaction().map_err(sql_error)?;
    transaction
        .execute_batch(include_str!("coordinator.sql"))
        .map_err(sql_error)?;
    transaction.commit().map_err(sql_error)?;
    migrate(connection)
}

fn has_table(connection: &Connection, name: &str) -> Result<bool, ControlStoreError> {
    Ok(connection
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1",
            [name],
            |_| Ok(()),
        )
        .optional()
        .map_err(sql_error)?
        .is_some())
}

fn migrate(connection: &mut Connection) -> Result<(), ControlStoreError> {
    let mut records = BTreeMap::new();
    read_filter(
        connection,
        &ControlFilter::all(ControlRecordKind::Project),
        &mut records,
    )?;
    for kind in PROJECT_KINDS {
        read_filter(connection, &ControlFilter::all(kind), &mut records)?;
    }
    let project_ids: Vec<_> = records
        .values()
        .filter(|record| record.kind == ControlRecordKind::Project)
        .map(|record| record.id.clone())
        .collect();
    let changes = records.into_values().map(ControlChange::Put).collect();
    let (mut commit, mut batches) = build_commit(connection, revision(connection)?, changes)?;
    commit.migration = true;
    for project_id in project_ids {
        ensure_target(connection, &mut commit, &project_id)?;
        batches.entry(project_id).or_default();
    }
    // Existing cursors remain stable across the physical split.
    for event in replay_locked(connection, 0, usize::MAX / 2)? {
        route_event(connection, &mut commit, &mut batches, event)?;
    }
    for checkpoint in progress_rows(connection)? {
        let project_id = checkpoint
            .body
            .get("project_id")
            .and_then(Value::as_str)
            .ok_or_else(|| other("legacy progress is missing project_id"))?
            .to_owned();
        ensure_target(connection, &mut commit, &project_id)?;
        batches
            .entry(project_id)
            .or_default()
            .progress
            .push(checkpoint);
    }
    submit(connection, &commit, &batches)
}

pub(super) fn finish(transaction: &Transaction<'_>) -> Result<(), ControlStoreError> {
    // The source remains intact until every Project has durably applied its batch.
    transaction
        .execute_batch(
            "PRAGMA defer_foreign_keys=ON;
         DROP TABLE workspace_run_journals;
         DROP TABLE run_credentials;
         DROP TABLE sessions;
         DROP TABLE runs;
         DROP TABLE messages;
         DROP TABLE run_progress;",
        )
        .map_err(sql_error)?;
    mark_format(transaction)
}

fn mark_format(transaction: &Transaction<'_>) -> Result<(), ControlStoreError> {
    transaction
        .execute("UPDATE split_metadata SET migrated=1 WHERE singleton=1", [])
        .map_err(sql_error)?;
    transaction
        .pragma_update(None, "application_id", GLOBAL_APPLICATION_ID)
        .map_err(sql_error)?;
    transaction
        .pragma_update(None, "user_version", 1)
        .map_err(sql_error)
}

fn backup_before_migration(connection: &Connection, path: &Path) -> Result<(), ControlStoreError> {
    let suffix: String = connection
        .query_row("SELECT lower(hex(randomblob(16)))", [], |row| row.get(0))
        .map_err(sql_error)?;
    let mut destination = path.as_os_str().to_os_string();
    destination.push(".pre-split.sqlite3");
    let mut destination = PathBuf::from(destination);
    if destination.try_exists().map_err(io_error)? {
        let mut unique = path.as_os_str().to_os_string();
        unique.push(format!(".{suffix}.pre-split.sqlite3"));
        destination = unique.into();
    }
    let mut temporary = path.as_os_str().to_os_string();
    temporary.push(format!(".{suffix}.pre-split.partial"));
    let temporary = PathBuf::from(temporary);
    let file = std::fs::File::create_new(&temporary).map_err(io_error)?;
    connection
        .backup(MAIN_DB, &temporary, None)
        .map_err(sql_error)?;
    file.sync_all().map_err(io_error)?;
    drop(file);
    std::fs::rename(&temporary, &destination).map_err(io_error)?;
    #[cfg(unix)]
    std::fs::File::open(
        path.parent()
            .ok_or_else(|| other("database parent is missing"))?,
    )
    .map_err(io_error)?
    .sync_all()
    .map_err(io_error)?;
    Ok(())
}
