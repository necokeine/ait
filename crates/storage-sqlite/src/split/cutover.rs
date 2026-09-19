//! One-time deletion of Ait conversation state. Native rollouts and files are untouched.
use super::{Connection, ControlStoreError, OptionalExtension, ProjectTarget, params, sql_error};

fn initialized(connection: &Connection) -> Result<bool, ControlStoreError> {
    connection.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='native_execution_cutover')", [], |row| row.get(0)).map_err(sql_error)
}

pub(super) fn initialize(connection: &mut Connection) -> Result<(), ControlStoreError> {
    if initialized(connection)? {
        return Ok(());
    }
    // Called under the coordinator lock, after all decided cross-file commits settle.
    // Offline Projects are recorded now and cleared on their next verified open.
    let transaction = connection.transaction().map_err(sql_error)?;
    transaction.execute_batch("CREATE TABLE native_execution_cutover (
        project_id TEXT PRIMARY KEY, workdir TEXT NOT NULL, root_message_id TEXT NOT NULL
    ) STRICT;
    INSERT INTO native_execution_cutover
        SELECT id, json_extract(body_json, '$.workdir'), json_extract(body_json, '$.root_message_id')
        FROM projects;
    DELETE FROM crons WHERE NOT EXISTS(SELECT 1 FROM native_execution_cutover c WHERE c.project_id=crons.project_id AND c.root_message_id=json_extract(crons.body_json, '$.base_message_id'));
    DELETE FROM agents WHERE json_extract(body_json, '$.owner_session_id') IS NOT NULL;
    DELETE FROM record_locations;
    INSERT INTO record_locations SELECT 'messages', root_message_id, project_id FROM native_execution_cutover;
    DELETE FROM durable_events;
    UPDATE control_metadata SET revision=revision+1 WHERE singleton=1 AND EXISTS(SELECT 1 FROM native_execution_cutover);
    PRAGMA user_version=2;") .map_err(sql_error)?;
    transaction.commit().map_err(sql_error)
}

pub(super) fn reset_root(
    connection: &Connection,
    target: &ProjectTarget,
) -> Result<Option<String>, ControlStoreError> {
    // Legacy layout recovery precedes creation of the cutover manifest.
    if !initialized(connection)? {
        return Ok(None);
    }
    connection.query_row("SELECT root_message_id FROM native_execution_cutover WHERE project_id=?1 AND workdir=?2",
        params![target.id, target.workdir], |row| row.get(0)).optional().map_err(sql_error)
}

pub(super) fn reset_project(
    connection: &mut Connection,
    target: &ProjectTarget,
) -> Result<(), ControlStoreError> {
    let Some(root) = &target.reset_root else {
        return Ok(());
    };
    if initialized(connection)? {
        return Ok(());
    }
    let transaction = connection.transaction().map_err(sql_error)?;
    let valid_root: bool = transaction.query_row(
        "SELECT EXISTS(SELECT 1 FROM messages WHERE id=?1 AND project_id=?2 AND parent_message_id IS NULL)",
        params![root, target.id], |row| row.get(0)).map_err(sql_error)?;
    if !valid_root {
        return Err(super::other(
            "cannot reset history without its verified Project root",
        ));
    }
    let prepared: bool = transaction
        .query_row("SELECT EXISTS(SELECT 1 FROM prepared_commit)", [], |row| {
            row.get(0)
        })
        .map_err(sql_error)?;
    if prepared {
        return Err(super::other(
            "cannot reset Project with an unresolved prepared commit",
        ));
    }
    transaction
        .execute_batch(
            "DELETE FROM workspace_run_journals;
        DELETE FROM run_credentials; DELETE FROM runs; DELETE FROM sessions;
        DELETE FROM run_progress; DELETE FROM durable_events;
        DROP TRIGGER messages_immutable_delete;",
        )
        .map_err(sql_error)?;
    transaction
        .execute("DELETE FROM messages WHERE id != ?1", [root])
        .map_err(sql_error)?;
    transaction
        .execute_batch(
            "CREATE TRIGGER messages_immutable_delete BEFORE DELETE ON messages BEGIN
        SELECT RAISE(ABORT, 'messages are immutable'); END;
        CREATE TABLE native_execution_cutover (version INTEGER NOT NULL CHECK(version=1)) STRICT;
        INSERT INTO native_execution_cutover VALUES(1);
        PRAGMA user_version=2;",
        )
        .map_err(sql_error)?;
    transaction.commit().map_err(sql_error)
}

#[cfg(test)]
mod tests;
