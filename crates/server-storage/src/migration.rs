use std::path::Path;

use rusqlite::{Connection, TransactionBehavior};
use server_ports::ProjectError;

use crate::sql;

pub(super) fn catalog(connection: &mut Connection, data_dir: &Path) -> Result<(), ProjectError> {
    let version: i32 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(|error| sql(&error))?;
    if version == 2 {
        return Ok(());
    }
    if version != 1 {
        return Err(ProjectError::UnsupportedFormat);
    }
    // The host holds the data-directory lease. Keep a completed, synced SQLite backup
    // before changing version 1; a migration failure preserves both original and backup.
    let backup = tempfile::Builder::new()
        .prefix("catalog-v1-backup-")
        .suffix(".sqlite3")
        .tempfile_in(data_dir)
        .map_err(|_| ProjectError::Io)?;
    connection
        .backup(rusqlite::MAIN_DB, backup.path(), None)
        .map_err(|error| sql(&error))?;
    backup.as_file().sync_all().map_err(|_| ProjectError::Io)?;
    let (_file, _path) = backup.keep().map_err(|_| ProjectError::Io)?;
    #[cfg(unix)]
    std::fs::File::open(data_dir)
        .and_then(|directory| directory.sync_all())
        .map_err(|_| ProjectError::Io)?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|error| sql(&error))?;
    transaction
        .execute_batch(crate::agents::SCHEMA)
        .map_err(|error| sql(&error))?;
    transaction
        .pragma_update(None, "user_version", 2)
        .map_err(|error| sql(&error))?;
    transaction.commit().map_err(|error| sql(&error))
}
