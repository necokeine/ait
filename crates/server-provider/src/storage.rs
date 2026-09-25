//! Agent preset SQLite catalog and Agent runtime snapshot storage.

pub mod agent_runtime;
mod agents;
mod catalog;
mod migration;

use std::path::Path;
use std::time::Duration;

use rusqlite::{Connection, OpenFlags};

use crate::ports::agent::AgentError;

pub use catalog::SqliteCatalog;

fn open(
    path: &Path,
    family: i32,
    schema: &str,
    latest_version: i32,
) -> Result<Connection, AgentError> {
    // Canonicalize the authorized directory, not the database file: macOS temporary
    // directories commonly pass through /var, while SQLite NOFOLLOW rejects aliases.
    // The final database/sidecar components must still be ordinary files.
    let path = path
        .parent()
        .ok_or(AgentError::Invalid)?
        .canonicalize()
        .map_err(|_| AgentError::Io)?
        .join(path.file_name().ok_or(AgentError::Invalid)?);
    for suffix in ["", "-journal", "-wal", "-shm"] {
        let mut file = path.as_os_str().to_owned();
        file.push(suffix);
        match std::fs::symlink_metadata(Path::new(&file)) {
            Ok(metadata) if !metadata.is_file() || metadata.file_type().is_symlink() => {
                return Err(AgentError::UnsupportedFormat);
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(AgentError::Io),
        }
    }
    let mut connection = Connection::open_with_flags(
        &path,
        OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_CREATE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .map_err(|error| sql(&error))?;
    connection
        .busy_timeout(Duration::from_secs(1))
        .map_err(|error| sql(&error))?;
    let application: i32 = connection
        .pragma_query_value(None, "application_id", |row| row.get(0))
        .map_err(|error| sql(&error))?;
    let version: i32 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(|error| sql(&error))?;
    if application == 0 && version == 0 {
        let objects: u64 = connection
            .query_row("SELECT count(*) FROM sqlite_schema", [], |row| row.get(0))
            .map_err(|error| sql(&error))?;
        if objects != 0 {
            return Err(AgentError::UnsupportedFormat);
        }
        let transaction = connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(|error| sql(&error))?;
        transaction
            .execute_batch(schema)
            .map_err(|error| sql(&error))?;
        transaction
            .pragma_update(None, "application_id", family)
            .map_err(|error| sql(&error))?;
        transaction
            .pragma_update(None, "user_version", latest_version)
            .map_err(|error| sql(&error))?;
        transaction.commit().map_err(|error| sql(&error))?;
    } else if application != family || !(1..=latest_version).contains(&version) {
        return Err(AgentError::UnsupportedFormat);
    }
    connection
        .execute_batch(
            "PRAGMA foreign_keys=ON; PRAGMA synchronous=FULL; PRAGMA trusted_schema=OFF;",
        )
        .map_err(|error| sql(&error))?;
    Ok(connection)
}

fn sql(error: &rusqlite::Error) -> AgentError {
    match error.sqlite_error_code() {
        Some(rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked) => {
            AgentError::Busy
        }
        Some(rusqlite::ErrorCode::NotADatabase | rusqlite::ErrorCode::DatabaseCorrupt) => {
            AgentError::UnsupportedFormat
        }
        Some(rusqlite::ErrorCode::ConstraintViolation) => AgentError::Invalid,
        _ => AgentError::Io,
    }
}

fn parse<T: std::str::FromStr>(row: &rusqlite::Row<'_>, index: usize) -> rusqlite::Result<T> {
    row.get::<_, String>(index)?
        .parse()
        .map_err(|_| rusqlite::Error::InvalidQuery)
}

#[cfg(test)]
mod tests;

pub mod timeline;
