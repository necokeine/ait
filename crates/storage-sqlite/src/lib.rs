//! `SQLite` adapters for global and per-project persistence.

use std::{path::Path, sync::Mutex};

use ait_ports::{
    ControlSnapshot, ControlStore, ControlStoreError, DurableEvent, DurableEventPage, EventBounds,
    MAX_TERMINAL_OUTPUT_ARCHIVE_BYTES, PendingEvent, ProgressCheckpoint, RunOutputArchive,
};
use async_trait::async_trait;
use rusqlite::{Connection, MAIN_DB, OptionalExtension, params};
use serde_json::Value;

const RETAINED_EVENTS: usize = 50_000;

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
             );
             CREATE TABLE IF NOT EXISTS run_progress (
               run_id TEXT PRIMARY KEY,
               body_json TEXT NOT NULL,
               updated_at INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS run_terminal_output (
               run_id TEXT PRIMARY KEY,
               body_json TEXT NOT NULL,
               updated_at INTEGER NOT NULL
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

    async fn commit_terminal(
        &self,
        expected_revision: u64,
        value: Value,
        events: Vec<PendingEvent>,
        run_id: &str,
        output: Option<RunOutputArchive>,
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
        transaction
            .execute(
                "INSERT INTO control_state(singleton, revision, body_json) VALUES(1, ?1, ?2)
                 ON CONFLICT(singleton) DO UPDATE SET revision = excluded.revision, body_json = excluded.body_json",
                params![revision, body],
            )
            .map_err(sql_error)?;
        for event in events {
            transaction
                .execute(
                    "INSERT INTO durable_events(kind, entity_id, body_json, created_at) VALUES(?1, ?2, ?3, ?4)",
                    params![event.kind, event.entity_id, serde_json::to_string(&event.body).map_err(json_error)?, event.created_at],
                )
                .map_err(sql_error)?;
        }
        if let Some(output) = output {
            if output.run_id != run_id {
                return Err(ControlStoreError::Other(
                    "terminal output Run identity does not match the transition".into(),
                ));
            }
            output
                .output
                .validate()
                .map_err(|error| ControlStoreError::Other(error.message))?;
            let output_body = serde_json::to_string(&output.output).map_err(json_error)?;
            transaction
                .execute(
                    "INSERT INTO run_terminal_output(run_id, body_json, updated_at) VALUES(?1, ?2, ?3)
                     ON CONFLICT(run_id) DO UPDATE SET body_json = excluded.body_json, updated_at = excluded.updated_at",
                    params![output.run_id, output_body, output.updated_at],
                )
                .map_err(sql_error)?;

            loop {
                let total = transaction
                    .query_row(
                        "SELECT COALESCE(SUM(length(CAST(body_json AS BLOB))), 0) FROM run_terminal_output",
                        [],
                        |row| row.get::<_, u64>(0),
                    )
                    .map_err(sql_error)?;
                if total <= u64::try_from(MAX_TERMINAL_OUTPUT_ARCHIVE_BYTES).unwrap_or(u64::MAX) {
                    break;
                }
                let oldest = transaction
                    .query_row(
                        "SELECT run_id FROM run_terminal_output WHERE run_id != ?1 ORDER BY updated_at, run_id LIMIT 1",
                        params![run_id],
                        |row| row.get::<_, String>(0),
                    )
                    .optional()
                    .map_err(sql_error)?
                    .ok_or_else(|| {
                        ControlStoreError::Other(
                            "terminal output archive exceeds its total byte budget".into(),
                        )
                    })?;
                transaction
                    .execute(
                        "DELETE FROM run_terminal_output WHERE run_id = ?1",
                        params![oldest],
                    )
                    .map_err(sql_error)?;
            }
        }
        transaction
            .execute(
                "DELETE FROM run_progress WHERE run_id = ?1",
                params![run_id],
            )
            .map_err(sql_error)?;
        transaction.commit().map_err(sql_error)?;
        Ok(ControlSnapshot { revision, value })
    }

    async fn replay(
        &self,
        cursor: u64,
        limit: usize,
    ) -> Result<Vec<DurableEvent>, ControlStoreError> {
        let connection = self.connection.lock().map_err(lock_error)?;
        replay_locked(&connection, cursor, limit)
    }

    async fn event_bounds(&self) -> Result<EventBounds, ControlStoreError> {
        let connection = self.connection.lock().map_err(lock_error)?;
        event_bounds_locked(&connection)
    }

    async fn replay_page(
        &self,
        cursor: u64,
        limit: usize,
    ) -> Result<DurableEventPage, ControlStoreError> {
        let connection = self.connection.lock().map_err(lock_error)?;
        let bounds = event_bounds_locked(&connection)?;
        let cursor_valid = cursor == 0
            || bounds
                .oldest
                .is_some_and(|oldest| cursor >= oldest.saturating_sub(1))
                && bounds.latest.is_some_and(|latest| cursor <= latest);
        let events = if cursor_valid {
            replay_locked(&connection, cursor, limit)?
        } else {
            Vec::new()
        };
        Ok(DurableEventPage {
            bounds,
            events,
            cursor_valid,
        })
    }

    async fn save_progress(
        &self,
        checkpoint: ProgressCheckpoint,
        events: Vec<PendingEvent>,
    ) -> Result<(), ControlStoreError> {
        let mut connection = self.connection.lock().map_err(lock_error)?;
        let transaction = connection.transaction().map_err(sql_error)?;
        for event in events {
            transaction.execute(
                "INSERT INTO durable_events(kind, entity_id, body_json, created_at) VALUES(?1, ?2, ?3, ?4)",
                params![event.kind, event.entity_id, serde_json::to_string(&event.body).map_err(json_error)?, event.created_at],
            ).map_err(sql_error)?;
        }
        transaction
            .execute(
                "INSERT INTO run_progress(run_id, body_json, updated_at) VALUES(?1, ?2, ?3)
                 ON CONFLICT(run_id) DO UPDATE SET body_json = excluded.body_json, updated_at = excluded.updated_at",
                params![
                    checkpoint.run_id,
                    serde_json::to_string(&checkpoint.body).map_err(json_error)?,
                    checkpoint.updated_at
                ],
            )
            .map_err(sql_error)?;
        transaction
            .execute(
                "DELETE FROM durable_events WHERE cursor < COALESCE(
                   (SELECT cursor FROM durable_events ORDER BY cursor DESC LIMIT 1 OFFSET ?1), 0
                 )",
                params![RETAINED_EVENTS.saturating_sub(1)],
            )
            .map_err(sql_error)?;
        transaction.commit().map_err(sql_error)
    }

    async fn load_progress(&self) -> Result<Vec<ProgressCheckpoint>, ControlStoreError> {
        let connection = self.connection.lock().map_err(lock_error)?;
        let mut statement = connection
            .prepare("SELECT run_id, body_json, updated_at FROM run_progress ORDER BY updated_at, run_id")
            .map_err(sql_error)?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            })
            .map_err(sql_error)?;
        rows.map(|row| {
            let (run_id, body, updated_at) = row.map_err(sql_error)?;
            Ok(ProgressCheckpoint {
                run_id,
                body: serde_json::from_str(&body).map_err(json_error)?,
                updated_at,
            })
        })
        .collect()
    }

    async fn load_run_outputs(
        &self,
        run_ids: &[String],
    ) -> Result<Vec<RunOutputArchive>, ControlStoreError> {
        let connection = self.connection.lock().map_err(lock_error)?;
        let mut statement = connection
            .prepare("SELECT body_json, updated_at FROM run_terminal_output WHERE run_id = ?1")
            .map_err(sql_error)?;
        let mut outputs = Vec::new();
        for run_id in run_ids {
            let row = statement
                .query_row(params![run_id], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
                })
                .optional()
                .map_err(sql_error)?;
            if let Some((body, updated_at)) = row {
                outputs.push(RunOutputArchive {
                    run_id: run_id.clone(),
                    output: serde_json::from_str(&body).map_err(json_error)?,
                    updated_at,
                });
            }
        }
        Ok(outputs)
    }

    async fn clear_progress(&self, run_id: &str) -> Result<(), ControlStoreError> {
        let connection = self.connection.lock().map_err(lock_error)?;
        connection
            .execute(
                "DELETE FROM run_progress WHERE run_id = ?1",
                params![run_id],
            )
            .map_err(sql_error)?;
        Ok(())
    }
}

fn event_bounds_locked(connection: &Connection) -> Result<EventBounds, ControlStoreError> {
    let (oldest, latest) = connection
        .query_row(
            "SELECT MIN(cursor), MAX(cursor) FROM durable_events",
            [],
            |row| Ok((row.get::<_, Option<u64>>(0)?, row.get::<_, Option<u64>>(1)?)),
        )
        .map_err(sql_error)?;
    Ok(EventBounds { oldest, latest })
}

fn replay_locked(
    connection: &Connection,
    cursor: u64,
    limit: usize,
) -> Result<Vec<DurableEvent>, ControlStoreError> {
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
    use std::sync::Arc;

    use super::*;

    #[tokio::test]
    async fn terminal_output_archive_is_independent_and_globally_bounded() {
        let store = SqliteControlStore::in_memory().unwrap();
        let mut revision = 0;
        let run_ids = (0..24)
            .map(|index| format!("run-{index:02}"))
            .collect::<Vec<_>>();
        for (index, run_id) in run_ids.iter().enumerate() {
            let output = serde_json::from_value(serde_json::json!({
                "progress": {"blob": "x".repeat(800 * 1024), "index": index}
            }))
            .unwrap();
            let value = serde_json::json!({"latest_run": run_id});
            let committed = store
                .commit_terminal(
                    revision,
                    value,
                    Vec::new(),
                    run_id,
                    Some(RunOutputArchive {
                        run_id: run_id.clone(),
                        output,
                        updated_at: i64::try_from(index).unwrap(),
                    }),
                )
                .await
                .unwrap();
            revision = committed.revision;
        }

        let outputs = store.load_run_outputs(&run_ids).await.unwrap();
        let total = outputs
            .iter()
            .map(|archive| serde_json::to_vec(&archive.output).unwrap().len())
            .sum::<usize>();
        assert!(total <= MAX_TERMINAL_OUTPUT_ARCHIVE_BYTES);
        assert!(outputs.len() < run_ids.len());
        assert!(outputs.iter().any(|archive| archive.run_id == "run-23"));
        let snapshot = store.load().await.unwrap();
        assert!(serde_json::to_vec(&snapshot.value).unwrap().len() < 1024);
    }

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

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn replay_boundary_is_contiguous_or_resets_during_concurrent_retention() {
        let store = Arc::new(SqliteControlStore::in_memory().unwrap());
        let initial = (0..=RETAINED_EVENTS)
            .map(|index| PendingEvent {
                kind: if index == 1 {
                    "run.updated".into()
                } else {
                    "run.progress".into()
                },
                entity_id: Some("run-a".into()),
                body: serde_json::json!({"index": index}),
                created_at: 1,
            })
            .collect();
        store
            .save_progress(
                ProgressCheckpoint {
                    run_id: "run-a".into(),
                    body: serde_json::json!({"seq": RETAINED_EVENTS}),
                    updated_at: 1,
                },
                initial,
            )
            .await
            .unwrap();
        assert_eq!(store.event_bounds().await.unwrap().oldest, Some(2));

        let barrier = Arc::new(tokio::sync::Barrier::new(2));
        let reader_store = store.clone();
        let reader_barrier = barrier.clone();
        let reader = tokio::spawn(async move {
            reader_barrier.wait().await;
            reader_store.replay_page(1, 8).await.unwrap()
        });
        let writer = tokio::spawn(async move {
            barrier.wait().await;
            store
                .save_progress(
                    ProgressCheckpoint {
                        run_id: "run-a".into(),
                        body: serde_json::json!({"seq": RETAINED_EVENTS + 64}),
                        updated_at: 2,
                    },
                    (0..64)
                        .map(|index| PendingEvent {
                            kind: "run.progress".into(),
                            entity_id: Some("run-a".into()),
                            body: serde_json::json!({"new": index}),
                            created_at: 2,
                        })
                        .collect(),
                )
                .await
                .unwrap();
        });

        let (page, writer) = tokio::join!(reader, writer);
        writer.unwrap();
        let page = page.unwrap();
        if page.cursor_valid {
            let first = page
                .events
                .first()
                .expect("valid page has the boundary event");
            assert_eq!(first.cursor, 2);
            assert_eq!(first.kind, "run.updated");
        } else {
            assert!(page.events.is_empty());
            assert!(page.bounds.oldest.is_some_and(|oldest| oldest > 2));
        }
    }
}
