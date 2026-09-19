//! Project discovery and acquisition. No catalog identity grants ownership.
use super::{
    BTreeMap, Connection, ControlFilter, ControlRecord, ControlStoreError, FORMAT, Kind,
    OptionalExtension, OwnedProject, PROJECT_ID, Path, ProjectVersion, State, catalog_id, io_error,
    other, ownership, params, random_id, read_filter, revision, sql_error,
};

pub(super) fn identity(root: &Path) -> Result<Option<String>, ControlStoreError> {
    let directory = root.join(".ait");
    ownership::reject_link(&directory)?;
    let path = directory.join("project.sqlite3");
    if !path.exists() {
        return Ok(None);
    }
    validate_files(root)?;
    let connection = Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(sql_error)?;
    let empty: bool = connection.query_row("SELECT NOT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%')",[],|row|row.get(0)).map_err(sql_error)?;
    if empty {
        return Ok(None);
    }
    verify_format(&connection)?;
    connection
        .query_row(
            "SELECT project_id FROM project_identity WHERE singleton=1",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(sql_error)
}

fn validate_files(root: &Path) -> Result<(), ControlStoreError> {
    ownership::reject_link(&root.join(".ait"))?;
    for suffix in ["", "-wal", "-shm", "-journal"] {
        ownership::reject_link(&root.join(".ait").join(format!("project.sqlite3{suffix}")))?;
    }
    Ok(())
}

fn verify_format(connection: &Connection) -> Result<(), ControlStoreError> {
    let format: u32 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(sql_error)?;
    let application: u32 = connection
        .pragma_query_value(None, "application_id", |row| row.get(0))
        .map_err(sql_error)?;
    if application != PROJECT_ID || format != FORMAT {
        return Err(other(
            "LEGACY_RECOVERY_REQUIRED: this Project must be converted with its original catalog before it can be opened",
        ));
    }
    Ok(())
}

pub(super) fn acquire(state: &mut State, id: &str, runtime: &str) -> Result<(), ControlStoreError> {
    if state.projects.contains_key(id) {
        validate_owned(&state.projects[id])?;
        return Ok(());
    }
    if state.closed.contains(id) {
        return Err(other(
            "PROJECT_CLOSED: explicitly open the Project before accessing its history",
        ));
    }
    let workdir: String = state
        .catalog
        .query_row(
            "SELECT json_extract(body_json, '$.workdir') FROM projects WHERE id=?1",
            [id],
            |row| row.get(0),
        )
        .map_err(sql_error)?;
    mount(state, id, Path::new(&workdir), runtime)
}

pub(super) fn validate_owned(owned: &OwnedProject) -> Result<(), ControlStoreError> {
    let canonical = std::fs::canonicalize(&owned.root).map_err(io_error)?;
    if canonical != owned.root || !owned.root.join(".ait/project.sqlite3").is_file() {
        return Err(other(
            "WORKSPACE_INVALID: the open Project directory moved or disappeared",
        ));
    }
    validate_files(&owned.root)?;
    owned.identity.verify(&owned.root)
}

pub(super) fn mount(
    state: &mut State,
    id: &str,
    root: &Path,
    runtime: &str,
) -> Result<(), ControlStoreError> {
    let root = std::fs::canonicalize(root).map_err(io_error)?;
    if let Some(owned) = state.projects.get(id) {
        return if owned.root == root {
            Ok(())
        } else {
            Err(other(
                "Project identity is already open at another directory",
            ))
        };
    }
    let locks = ownership::acquire(&root, id)?;
    validate_files(&root)?;
    let path = root.join(".ait/project.sqlite3");
    let existed = path.exists();
    if !existed {
        return Err(other(
            "Project database is unavailable; it will not be recreated",
        ));
    }
    let connection =
        Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE)
            .map_err(sql_error)?;
    connection
        .busy_timeout(std::time::Duration::from_secs(5))
        .map_err(sql_error)?;
    verify_format(&connection)?;
    connection
        .execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; PRAGMA foreign_keys=ON;")
        .map_err(sql_error)?;
    verify_format(&connection)?;
    let saved: String = connection
        .query_row(
            "SELECT project_id FROM project_identity WHERE singleton=1",
            [],
            |row| row.get(0),
        )
        .map_err(sql_error)?;
    if saved != id {
        return Err(other("Project identity does not match this directory"));
    }
    check_stream(&state.catalog, &connection, id)?;
    connection.execute("UPDATE project_identity SET owner_epoch=owner_epoch+1, owner_instance=?1 WHERE singleton=1 AND owner_epoch < 9223372036854775807", [runtime]).map_err(sql_error)?;
    let epoch = version(&connection)?;
    if epoch.runtime_instance_id != runtime {
        return Err(other("Project ownership generation exhausted"));
    }
    state.projects.insert(
        id.to_owned(),
        OwnedProject {
            connection,
            identity: ownership::FileIdentity::capture(&root)?,
            root,
            _locks: locks,
        },
    );
    state.closed.remove(id);
    Ok(())
}

pub(super) fn create(
    state: &mut State,
    id: &str,
    root: &Path,
    runtime: &str,
    initialize: impl FnOnce(&Connection, &rusqlite::Transaction<'_>) -> Result<u64, ControlStoreError>,
) -> Result<u64, ControlStoreError> {
    let root = std::fs::canonicalize(root).map_err(io_error)?;
    super::super::split::project::exclude_history(&root)?;
    let locks = ownership::acquire(&root, id)?;
    validate_files(&root)?;
    let mut connection = Connection::open(root.join(".ait/project.sqlite3")).map_err(sql_error)?;
    let tables: u64 = connection
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'",
            [],
            |row| row.get(0),
        )
        .map_err(sql_error)?;
    if tables != 0 {
        return Err(other(
            "Existing Project storage must be opened, not initialized again",
        ));
    }
    connection
        .execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; PRAGMA foreign_keys=ON;")
        .map_err(sql_error)?;
    let transaction = connection.transaction().map_err(sql_error)?;
    transaction
        .execute_batch(include_str!("project.sql"))
        .map_err(sql_error)?;
    transaction
        .execute(
            "INSERT INTO project_identity VALUES(1,?1,?2,1,?3,?4)",
            params![
                id,
                random_id(&transaction)?,
                runtime,
                catalog_id(&state.catalog)?
            ],
        )
        .map_err(sql_error)?;
    transaction
        .pragma_update(None, "application_id", PROJECT_ID)
        .map_err(sql_error)?;
    transaction
        .pragma_update(None, "user_version", FORMAT)
        .map_err(sql_error)?;
    let next = initialize(&state.catalog, &transaction)?;
    transaction.commit().map_err(sql_error)?;
    state.projects.insert(
        id.to_owned(),
        OwnedProject {
            connection,
            identity: ownership::FileIdentity::capture(&root)?,
            root,
            _locks: locks,
        },
    );
    state.closed.remove(id);
    Ok(next)
}

pub(super) fn version(connection: &Connection) -> Result<ProjectVersion, ControlStoreError> {
    let (runtime_instance_id, owner_epoch) = connection
        .query_row(
            "SELECT owner_instance, owner_epoch FROM project_identity WHERE singleton=1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(sql_error)?;
    Ok(ProjectVersion {
        runtime_instance_id,
        owner_epoch,
        revision: revision(connection)?,
    })
}

pub(super) fn record(owned: &OwnedProject, id: &str) -> Result<ControlRecord, ControlStoreError> {
    let mut records = BTreeMap::new();
    read_filter(
        &owned.connection,
        &ControlFilter::id(Kind::Project, id),
        &mut records,
    )?;
    let mut record = records
        .remove(&(Kind::Project, id.to_owned()))
        .ok_or_else(|| other("Project initialization is incomplete"))?;
    record.value["workdir"] = serde_json::json!(owned.root);
    record.value["execution_blocked"] = super::workers::assert_prior_quiescent(&owned.connection)
        .err()
        .map_or(serde_json::Value::Null, |error| {
            serde_json::Value::String(error.to_string())
        });
    record.value["owner"] =
        serde_json::to_value(version(&owned.connection)?.owner(id)).map_err(super::json_error)?;
    Ok(record)
}

pub(super) fn check_stream(
    catalog: &Connection,
    local: &Connection,
    id: &str,
) -> Result<(), ControlStoreError> {
    let observed: Option<(String, u64)> = catalog
        .query_row(
            "SELECT stream_id,revision FROM projection_watermarks WHERE project_id=?1",
            [id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(sql_error)?;
    if let Some((stream, observed_revision)) = observed {
        let current: String = local
            .query_row(
                "SELECT stream_id FROM project_identity WHERE singleton=1",
                [],
                |row| row.get(0),
            )
            .map_err(sql_error)?;
        if stream == current && revision(local)? < observed_revision {
            return Err(other(
                "RESTORE_REQUIRED: Project revision moved backwards; restore requires a new event stream",
            ));
        }
    }
    Ok(())
}
