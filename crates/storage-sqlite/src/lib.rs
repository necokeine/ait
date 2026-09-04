//! `SQLite` adapters for global and per-project persistence.

use std::{path::Path, sync::Mutex};

use ait_ports::{ControlSnapshot, ControlStore, ControlStoreError, DurableEvent, PendingEvent};
use async_trait::async_trait;
use rusqlite::{Connection, MAIN_DB, OptionalExtension, params};
use serde_json::Value;

/// SQLite-backed application snapshot and transactional durable event outbox.
pub struct SqliteControlStore {
    connection: Mutex<Connection>,
}

impl SqliteControlStore {
    /// Opens or creates a store and applies its idempotent schema.
    ///
    /// # Errors
    ///
    /// Returns a safe adapter error when `SQLite` cannot be opened or initialized.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, ControlStoreError> {
        let connection = Connection::open(path).map_err(sql_error)?;
        Self::initialize(connection)
    }

    /// Creates an isolated in-memory store for tests and embedded callers.
    ///
    /// # Errors
    ///
    /// Returns a safe adapter error when `SQLite` initialization fails.
    pub fn in_memory() -> Result<Self, ControlStoreError> {
        Self::initialize(Connection::open_in_memory().map_err(sql_error)?)
    }

    fn initialize(connection: Connection) -> Result<Self, ControlStoreError> {
        connection
            .execute_batch(
                "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = FULL;
             PRAGMA wal_autocheckpoint = 1000;
             PRAGMA foreign_keys = ON;
             CREATE TABLE IF NOT EXISTS control_state (
               singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
               revision INTEGER NOT NULL,
               body_json TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS durable_events (
               cursor INTEGER PRIMARY KEY AUTOINCREMENT,
               kind TEXT NOT NULL,
               entity_id TEXT,
               body_json TEXT NOT NULL,
               created_at INTEGER NOT NULL
             );",
            )
            .map_err(sql_error)?;
        Ok(Self {
            connection: Mutex::new(connection),
        })
    }

    /// Creates a transactionally consistent online backup.
    ///
    /// Provider credentials and external secret stores are not part of this
    /// `SQLite` archive.
    ///
    /// # Errors
    ///
    /// Returns a safe adapter error if the source cannot be locked or copied.
    pub fn backup_to(&self, destination: impl AsRef<Path>) -> Result<(), ControlStoreError> {
        let connection = self.connection.lock().map_err(lock_error)?;
        connection
            .backup(MAIN_DB, destination, None)
            .map_err(sql_error)
    }

    /// Restores an online backup into this open store.
    ///
    /// Callers should stop request processing before restore so successful
    /// post-backup writes are not intentionally discarded.
    ///
    /// # Errors
    ///
    /// Returns a safe adapter error if the source is invalid or restore fails.
    pub fn restore_from(&self, source: impl AsRef<Path>) -> Result<(), ControlStoreError> {
        let mut connection = self.connection.lock().map_err(lock_error)?;
        connection
            .restore(MAIN_DB, source, None::<fn(rusqlite::backup::Progress)>)
            .map_err(sql_error)
    }

    /// Runs `SQLite`'s fast structural integrity check.
    ///
    /// # Errors
    ///
    /// Returns a safe adapter error when the check cannot run or reports damage.
    pub fn quick_check(&self) -> Result<(), ControlStoreError> {
        let connection = self.connection.lock().map_err(lock_error)?;
        let result = connection
            .query_row("PRAGMA quick_check", [], |row| row.get::<_, String>(0))
            .map_err(sql_error)?;
        if result == "ok" {
            Ok(())
        } else {
            Err(ControlStoreError::Other(format!(
                "SQLite quick_check failed: {result}"
            )))
        }
    }
}

#[async_trait]
impl ControlStore for SqliteControlStore {
    async fn load(&self) -> Result<ControlSnapshot, ControlStoreError> {
        let connection = self.connection.lock().map_err(lock_error)?;
        connection
            .query_row(
                "SELECT revision, body_json FROM control_state WHERE singleton = 1",
                [],
                |row| Ok((row.get::<_, u64>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()
            .map_err(sql_error)?
            .map_or_else(
                || {
                    Ok(ControlSnapshot {
                        revision: 0,
                        value: Value::Null,
                    })
                },
                |(revision, body)| {
                    Ok(ControlSnapshot {
                        revision,
                        value: serde_json::from_str(&body).map_err(json_error)?,
                    })
                },
            )
    }

    async fn commit(
        &self,
        expected_revision: u64,
        value: Value,
        events: Vec<PendingEvent>,
    ) -> Result<ControlSnapshot, ControlStoreError> {
        let mut connection = self.connection.lock().map_err(lock_error)?;
        let transaction = connection.transaction().map_err(sql_error)?;
        let current = transaction
            .query_row(
                "SELECT revision FROM control_state WHERE singleton = 1",
                [],
                |row| row.get::<_, u64>(0),
            )
            .optional()
            .map_err(sql_error)?
            .unwrap_or(0);
        if current != expected_revision {
            return Err(ControlStoreError::Conflict);
        }
        let revision = expected_revision.saturating_add(1);
        let body = serde_json::to_string(&value).map_err(json_error)?;
        transaction.execute(
            "INSERT INTO control_state(singleton, revision, body_json) VALUES(1, ?1, ?2)
             ON CONFLICT(singleton) DO UPDATE SET revision = excluded.revision, body_json = excluded.body_json",
            params![revision, body],
        ).map_err(sql_error)?;
        for event in events {
            transaction.execute(
                "INSERT INTO durable_events(kind, entity_id, body_json, created_at) VALUES(?1, ?2, ?3, ?4)",
                params![event.kind, event.entity_id, serde_json::to_string(&event.body).map_err(json_error)?, event.created_at],
            ).map_err(sql_error)?;
        }
        transaction.commit().map_err(sql_error)?;
        Ok(ControlSnapshot { revision, value })
    }

    async fn replay(
        &self,
        cursor: u64,
        limit: usize,
    ) -> Result<Vec<DurableEvent>, ControlStoreError> {
        let connection = self.connection.lock().map_err(lock_error)?;
        let mut statement = connection.prepare(
            "SELECT cursor, kind, entity_id, body_json, created_at FROM durable_events WHERE cursor > ?1 ORDER BY cursor LIMIT ?2",
        ).map_err(sql_error)?;
        let rows = statement
            .query_map(params![cursor, limit], |row| {
                Ok((
                    row.get::<_, u64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, i64>(4)?,
                ))
            })
            .map_err(sql_error)?;
        rows.map(|row| {
            let (cursor, kind, entity_id, body, created_at) = row.map_err(sql_error)?;
            Ok(DurableEvent {
                cursor,
                kind,
                entity_id,
                body: serde_json::from_str(&body).map_err(json_error)?,
                created_at,
            })
        })
        .collect()
    }
}

#[allow(clippy::needless_pass_by_value)]
fn sql_error(error: rusqlite::Error) -> ControlStoreError {
    ControlStoreError::Other(error.to_string())
}
#[allow(clippy::needless_pass_by_value)]
fn json_error(error: serde_json::Error) -> ControlStoreError {
    ControlStoreError::Other(error.to_string())
}
#[allow(clippy::needless_pass_by_value)]
fn lock_error<T>(error: std::sync::PoisonError<T>) -> ControlStoreError {
    ControlStoreError::Other(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn online_backup_restores_a_consistent_revision_and_outbox() {
        let temporary = tempfile::TempDir::new().unwrap();
        let database = temporary.path().join("live.sqlite3");
        let backup = temporary.path().join("backup.sqlite3");
        let store = SqliteControlStore::open(&database).unwrap();
        store
            .commit(
                0,
                serde_json::json!({"state": "backed-up"}),
                vec![PendingEvent {
                    kind: "test.committed".into(),
                    entity_id: Some("p1".into()),
                    body: serde_json::json!({"revision": 1}),
                    created_at: 1,
                }],
            )
            .await
            .unwrap();
        store.backup_to(&backup).unwrap();
        store
            .commit(1, serde_json::json!({"state": "newer"}), Vec::new())
            .await
            .unwrap();

        store.restore_from(&backup).unwrap();
        store.quick_check().unwrap();
        let recovered = store.load().await.unwrap();
        assert_eq!(recovered.revision, 1);
        assert_eq!(recovered.value["state"], "backed-up");
        assert_eq!(store.replay(0, 10).await.unwrap().len(), 1);
    }
}
