//! `SQLite` adapters for global and per-project persistence.

use std::{path::Path, sync::Mutex};

use ait_ports::{
    ControlSnapshot, ControlStore, ControlStoreError, DurableEvent, DurableEventPage, EventBounds,
    MAX_TERMINAL_OUTPUT_ARCHIVE_BYTES, PendingEvent, ProgressCheckpoint, RunOutputArchive,
};
use async_trait::async_trait;
use rusqlite::{Connection, MAIN_DB, OptionalExtension, Transaction, params};
use serde_json::Value;

const RETAINED_EVENTS: usize = 50_000;
const MAX_TERMINAL_EVENT_BODY_BYTES: usize = 64 * 1024;
const MAX_TERMINAL_OUTPUT_ARCHIVES: usize = 1_024;

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
        append_retained_events(&transaction, events)?;
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
        if events.iter().any(|event| {
            serde_json::to_vec(&event.body)
                .map_or(true, |body| body.len() > MAX_TERMINAL_EVENT_BODY_BYTES)
        }) {
            return Err(ControlStoreError::Other(
                "terminal Run event exceeds its lightweight body budget".into(),
            ));
        }
        append_retained_events(&transaction, events)?;
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
        }

        prune_terminal_output_archives(&transaction, run_id)?;
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
        append_retained_events(&transaction, events)?;
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
        let requested = serde_json::to_string(run_ids).map_err(json_error)?;
        let query = if run_ids.is_empty() {
            "SELECT run_id, body_json, updated_at
             FROM run_terminal_output
             WHERE length(?1) >= 0
             ORDER BY updated_at, run_id"
        } else {
            "SELECT run_id, body_json, updated_at
             FROM run_terminal_output
             WHERE run_id IN (SELECT value FROM json_each(?1))
             ORDER BY updated_at, run_id"
        };
        let mut statement = connection.prepare(query).map_err(sql_error)?;
        let rows = statement
            .query_map(params![requested], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            })
            .map_err(sql_error)?;
        rows.map(|row| {
            let (run_id, body, updated_at) = row.map_err(sql_error)?;
            Ok(RunOutputArchive {
                run_id,
                output: serde_json::from_str(&body).map_err(json_error)?,
                updated_at,
            })
        })
        .collect()
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

fn prune_terminal_output_archives(
    transaction: &Transaction<'_>,
    retained_run_id: &str,
) -> Result<(), ControlStoreError> {
    // Enforce the row-count budget in one statement so the first terminal
    // write after upgrading an old database is not O(n²).
    transaction
        .execute(
            "DELETE FROM run_terminal_output
             WHERE run_id IN (
               SELECT run_id FROM run_terminal_output
               WHERE run_id != ?1
               ORDER BY updated_at DESC, run_id DESC
               LIMIT -1 OFFSET ?2
             )",
            params![
                retained_run_id,
                MAX_TERMINAL_OUTPUT_ARCHIVES.saturating_sub(1)
            ],
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
            return Ok(());
        }
        let oldest = transaction
            .query_row(
                "SELECT run_id FROM run_terminal_output WHERE run_id != ?1 ORDER BY updated_at, run_id LIMIT 1",
                params![retained_run_id],
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

fn append_retained_events(
    transaction: &Transaction<'_>,
    events: Vec<PendingEvent>,
) -> Result<(), ControlStoreError> {
    for event in events {
        transaction.execute(
            "INSERT INTO durable_events(kind, entity_id, body_json, created_at) VALUES(?1, ?2, ?3, ?4)",
            params![event.kind, event.entity_id, serde_json::to_string(&event.body).map_err(json_error)?, event.created_at],
        ).map_err(sql_error)?;
    }
    transaction
        .execute(
            "DELETE FROM durable_events WHERE cursor < COALESCE(
               (SELECT cursor FROM durable_events ORDER BY cursor DESC LIMIT 1 OFFSET ?1), 0
             )",
            params![RETAINED_EVENTS.saturating_sub(1)],
        )
        .map_err(sql_error)?;
    Ok(())
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
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    use super::*;
    use rusqlite::trace::{TraceEvent, TraceEventCodes};

    #[tokio::test]
    async fn terminal_output_archive_is_independent_and_globally_bounded() {
        let temporary = tempfile::TempDir::new().unwrap();
        let database = temporary.path().join("bounded.sqlite3");
        let store = SqliteControlStore::open(&database).unwrap();
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
                    vec![PendingEvent {
                        kind: "run.updated".into(),
                        entity_id: Some(run_id.clone()),
                        body: serde_json::json!({
                            "version": 1,
                            "id": run_id,
                            "status": "failed",
                            "partial_output_available": true,
                        }),
                        created_at: i64::try_from(index).unwrap(),
                    }],
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

        let outputs = store.load_run_outputs(&[]).await.unwrap();
        let total = outputs
            .iter()
            .map(|archive| serde_json::to_vec(&archive.output).unwrap().len())
            .sum::<usize>();
        assert!(total <= MAX_TERMINAL_OUTPUT_ARCHIVE_BYTES);
        assert!(outputs.len() < run_ids.len());
        assert!(outputs.iter().any(|archive| archive.run_id == "run-23"));
        let snapshot = store.load().await.unwrap();
        assert!(serde_json::to_vec(&snapshot.value).unwrap().len() < 1024);
        let connection = store.connection.lock().unwrap();
        let event_bytes = connection
            .query_row(
                "SELECT COALESCE(SUM(length(CAST(body_json AS BLOB))), 0) FROM durable_events",
                [],
                |row| row.get::<_, usize>(0),
            )
            .unwrap();
        let page_count = connection
            .query_row("PRAGMA page_count", [], |row| row.get::<_, usize>(0))
            .unwrap();
        let page_size = connection
            .query_row("PRAGMA page_size", [], |row| row.get::<_, usize>(0))
            .unwrap();
        assert!(event_bytes < 64 * 1024);
        assert!(page_count * page_size < MAX_TERMINAL_OUTPUT_ARCHIVE_BYTES * 2);
    }

    static OUTPUT_SELECTS: AtomicUsize = AtomicUsize::new(0);

    fn count_output_selects(event: TraceEvent<'_>) {
        if let TraceEvent::Stmt(statement, _) = event {
            let sql = statement.sql();
            if sql.starts_with("SELECT run_id, body_json, updated_at")
                && sql.contains("FROM run_terminal_output")
            {
                OUTPUT_SELECTS.fetch_add(1, Ordering::SeqCst);
            }
        }
    }

    #[tokio::test]
    async fn bounded_run_output_catalog_is_hydrated_with_one_query() {
        let store = SqliteControlStore::in_memory().unwrap();
        let run_ids = (0..MAX_TERMINAL_OUTPUT_ARCHIVES + 32)
            .map(|index| format!("batch-run-{index}"))
            .collect::<Vec<_>>();
        let mut revision = 0;
        for (index, run_id) in run_ids.iter().enumerate() {
            revision = store
                .commit_terminal(
                    revision,
                    serde_json::json!({"run": run_id}),
                    Vec::new(),
                    run_id,
                    Some(RunOutputArchive {
                        run_id: run_id.clone(),
                        output: serde_json::from_value(serde_json::json!({
                            "progress": {"index": index}
                        }))
                        .unwrap(),
                        updated_at: i64::try_from(index).unwrap(),
                    }),
                )
                .await
                .unwrap()
                .revision;
        }

        OUTPUT_SELECTS.store(0, Ordering::SeqCst);
        store.connection.lock().unwrap().trace_v2(
            TraceEventCodes::SQLITE_TRACE_STMT,
            Some(count_output_selects),
        );
        let outputs = store.load_run_outputs(&[]).await.unwrap();
        store
            .connection
            .lock()
            .unwrap()
            .trace_v2(TraceEventCodes::empty(), None);

        assert_eq!(outputs.len(), MAX_TERMINAL_OUTPUT_ARCHIVES);
        assert_eq!(outputs.first().unwrap().run_id, "batch-run-32");
        assert_eq!(OUTPUT_SELECTS.load(Ordering::SeqCst), 1);
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

    #[tokio::test]
    async fn ordinary_commits_preserve_the_global_outbox_retention_bound() {
        let store = SqliteControlStore::in_memory().unwrap();
        let progress = (0..RETAINED_EVENTS)
            .map(|index| PendingEvent {
                kind: "run.progress".into(),
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
                progress,
            )
            .await
            .unwrap();

        for (revision, kind) in ["session.updated", "run.updated"].into_iter().enumerate() {
            store
                .commit(
                    u64::try_from(revision).unwrap(),
                    serde_json::json!({"revision": revision + 1}),
                    vec![PendingEvent {
                        kind: kind.into(),
                        entity_id: Some("run-a".into()),
                        body: serde_json::json!({"status": "completed"}),
                        created_at: i64::try_from(revision + 2).unwrap(),
                    }],
                )
                .await
                .unwrap();

            let events = store.replay(0, RETAINED_EVENTS + 1).await.unwrap();
            assert_eq!(events.len(), RETAINED_EVENTS);
            let bounds = store.event_bounds().await.unwrap();
            assert_eq!(bounds.oldest, Some(u64::try_from(revision + 2).unwrap()));
            assert_eq!(
                bounds.latest,
                Some(u64::try_from(RETAINED_EVENTS + revision + 1).unwrap())
            );
            assert_eq!(events.first().map(|event| event.cursor), bounds.oldest);
            assert_eq!(events.last().map(|event| event.cursor), bounds.latest);
        }
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
