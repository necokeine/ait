//! Conservative host-wide Thread reservations. Unknown native-store identities
//! share one collision domain; they never authorize cross-catalog native continuation.
use super::{
    Connection, ControlChange, ControlStoreError, Kind, OptionalExtension, Value, catalog_lock,
    io_error, other, ownership, params, sql_error,
};

pub(super) fn reserve(
    project: &rusqlite::Transaction<'_>,
    changes: &[ControlChange],
) -> Result<(), ControlStoreError> {
    for change in changes {
        let ControlChange::Put(record) = change else {
            continue;
        };
        if record.kind != Kind::Session
            || record.value.pointer("/source/type").and_then(Value::as_str) != Some("codex_thread")
        {
            continue;
        }
        let thread = record
            .value
            .pointer("/source/thread_id")
            .and_then(Value::as_str)
            .ok_or_else(|| other("Native Session has no Thread identity"))?;
        let project_id = record
            .project_id
            .as_deref()
            .ok_or_else(|| other("Native Session has no Project identity"))?;
        claim(thread, project_id, &record.id)?;
        project
            .execute(
                "INSERT OR IGNORE INTO native_bindings VALUES(?1,?2)",
                params![thread, record.id],
            )
            .map_err(sql_error)?;
    }
    Ok(())
}

fn claim(thread: &str, project_id: &str, session_id: &str) -> Result<(), ControlStoreError> {
    let root = ownership::common_directory()?;
    let _lock = catalog_lock(&root.join("native-bindings.lock"))?;
    let path = root.join("native-bindings.sqlite3");
    let marker = root.join("native-bindings.initialized");
    ownership::reject_link(&path)?;
    ownership::reject_link(&marker)?;
    if marker.exists() && !path.exists() {
        return Err(other(
            "BINDING_UNKNOWN: native ownership registry is missing; restore its backup before importing Threads",
        ));
    }
    let connection = Connection::open(path).map_err(sql_error)?;
    connection.execute_batch("PRAGMA synchronous=FULL; CREATE TABLE IF NOT EXISTS bindings(thread_id TEXT PRIMARY KEY,project_id TEXT NOT NULL,session_id TEXT NOT NULL) STRICT;").map_err(sql_error)?;
    if !marker.exists() {
        std::fs::File::create_new(marker)
            .map_err(io_error)?
            .sync_all()
            .map_err(io_error)?;
    }
    let existing: Option<(String, String)> = connection
        .query_row(
            "SELECT project_id,session_id FROM bindings WHERE thread_id=?1",
            [thread],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(sql_error)?;
    if let Some((saved_project, saved_session)) = existing {
        if saved_project != project_id || saved_session != session_id {
            return Err(other(
                "NATIVE_BINDING_CONFLICT: this Thread is already reserved by another Project or Session on this host",
            ));
        }
    } else {
        connection
            .execute(
                "INSERT INTO bindings VALUES(?1,?2,?3)",
                params![thread, project_id, session_id],
            )
            .map_err(sql_error)?;
    }
    Ok(())
}
