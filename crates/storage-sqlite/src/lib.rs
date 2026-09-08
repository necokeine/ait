//! Record-oriented `SQLite` persistence for the local control plane.

use std::{collections::BTreeMap, path::Path, sync::Mutex};

use ait_ports::{
    ControlChange, ControlFilter, ControlRead, ControlRecord, ControlRecordKind, ControlStore,
    ControlStoreError, DurableEvent, DurableEventPage, EventBounds, PendingEvent,
    ProgressCheckpoint,
};
use async_trait::async_trait;
use rusqlite::{Connection, MAIN_DB, OptionalExtension, Transaction, params};
use serde_json::Value;

const RETAINED_EVENTS: usize = 50_000;
const RECORD_SCHEMA: &str = "PRAGMA journal_mode = WAL;
                 PRAGMA synchronous = FULL;
                 PRAGMA wal_autocheckpoint = 1000;
                 PRAGMA foreign_keys = ON;
                 CREATE TABLE IF NOT EXISTS control_metadata (
                   singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
                   revision INTEGER NOT NULL
                 ) STRICT;
                 INSERT OR IGNORE INTO control_metadata(singleton, revision) VALUES(1, 0);

                 CREATE TABLE IF NOT EXISTS projects (
                   id TEXT PRIMARY KEY,
                   project_id TEXT NOT NULL CHECK (project_id = id),
                   body_json TEXT NOT NULL CHECK (json_valid(body_json))
                 ) STRICT;
                 CREATE UNIQUE INDEX IF NOT EXISTS projects_workdir_unique
                   ON projects(json_extract(body_json, '$.workdir'));
                 CREATE TABLE IF NOT EXISTS agents (
                   id TEXT PRIMARY KEY,
                   project_id TEXT,
                   body_json TEXT NOT NULL CHECK (json_valid(body_json))
                 ) STRICT;
                 CREATE INDEX IF NOT EXISTS agents_provider
                   ON agents(json_extract(body_json, '$.config.provider_id'));
                 CREATE TABLE IF NOT EXISTS agent_providers (
                   id TEXT PRIMARY KEY,
                   project_id TEXT,
                   body_json TEXT NOT NULL CHECK (json_valid(body_json))
                 ) STRICT;
                 CREATE TABLE IF NOT EXISTS provider_credentials (
                   id TEXT PRIMARY KEY,
                   project_id TEXT,
                   body_json TEXT NOT NULL CHECK (json_valid(body_json))
                 ) STRICT;
                 CREATE TABLE IF NOT EXISTS messages (
                   id TEXT PRIMARY KEY,
                   project_id TEXT NOT NULL REFERENCES projects(id) DEFERRABLE INITIALLY DEFERRED,
                   parent_message_id TEXT REFERENCES messages(id) DEFERRABLE INITIALLY DEFERRED,
                   body_json TEXT NOT NULL CHECK (json_valid(body_json))
                 ) STRICT;
                 CREATE INDEX IF NOT EXISTS messages_project_parent
                   ON messages(project_id, parent_message_id);
                 CREATE TABLE IF NOT EXISTS sessions (
                   id TEXT PRIMARY KEY,
                   project_id TEXT NOT NULL REFERENCES projects(id) DEFERRABLE INITIALLY DEFERRED,
                   body_json TEXT NOT NULL CHECK (json_valid(body_json))
                 ) STRICT;
                 CREATE INDEX IF NOT EXISTS sessions_project ON sessions(project_id);
                 CREATE TABLE IF NOT EXISTS runs (
                   id TEXT PRIMARY KEY,
                   project_id TEXT NOT NULL REFERENCES projects(id) DEFERRABLE INITIALLY DEFERRED,
                   body_json TEXT NOT NULL CHECK (json_valid(body_json))
                 ) STRICT;
                 CREATE INDEX IF NOT EXISTS runs_project ON runs(project_id);
                 CREATE INDEX IF NOT EXISTS runs_session
                   ON runs(json_extract(body_json, '$.session_id'));
                 CREATE INDEX IF NOT EXISTS runs_cron
                   ON runs(json_extract(body_json, '$.cron_id'));
                 CREATE TABLE IF NOT EXISTS run_credentials (
                   id TEXT PRIMARY KEY,
                   project_id TEXT NOT NULL REFERENCES projects(id) DEFERRABLE INITIALLY DEFERRED,
                   body_json TEXT NOT NULL CHECK (json_valid(body_json))
                 ) STRICT;
                 CREATE INDEX IF NOT EXISTS run_credentials_project
                   ON run_credentials(project_id);
                 CREATE TABLE IF NOT EXISTS workspace_run_journals (
                   id TEXT PRIMARY KEY REFERENCES runs(id) DEFERRABLE INITIALLY DEFERRED,
                   project_id TEXT NOT NULL REFERENCES projects(id) DEFERRABLE INITIALLY DEFERRED,
                   body_json TEXT NOT NULL CHECK (json_valid(body_json))
                 ) STRICT;
                 CREATE INDEX IF NOT EXISTS workspace_run_journals_project
                   ON workspace_run_journals(project_id);
                 CREATE TABLE IF NOT EXISTS crons (
                   id TEXT PRIMARY KEY,
                   project_id TEXT NOT NULL REFERENCES projects(id) DEFERRABLE INITIALLY DEFERRED,
                   body_json TEXT NOT NULL CHECK (json_valid(body_json))
                 ) STRICT;
                 CREATE INDEX IF NOT EXISTS crons_project ON crons(project_id);
                 CREATE TABLE IF NOT EXISTS settings (
                   id TEXT PRIMARY KEY CHECK (id = 'settings'),
                   project_id TEXT,
                   body_json TEXT NOT NULL CHECK (json_valid(body_json))
                 ) STRICT;

                 CREATE TRIGGER IF NOT EXISTS messages_immutable_update
                 BEFORE UPDATE ON messages BEGIN
                   SELECT RAISE(ABORT, 'messages are immutable');
                 END;
                 CREATE TRIGGER IF NOT EXISTS messages_immutable_delete
                 BEFORE DELETE ON messages BEGIN
                   SELECT RAISE(ABORT, 'messages are immutable');
                 END;

                 CREATE TABLE IF NOT EXISTS durable_events (
                   cursor INTEGER PRIMARY KEY AUTOINCREMENT,
                   kind TEXT NOT NULL,
                   entity_id TEXT,
                   body_json TEXT NOT NULL CHECK (json_valid(body_json)),
                   created_at INTEGER NOT NULL
                 ) STRICT;
                 CREATE TABLE IF NOT EXISTS run_progress (
                   run_id TEXT PRIMARY KEY,
                   body_json TEXT NOT NULL CHECK (json_valid(body_json)),
                   updated_at INTEGER NOT NULL
                 ) STRICT;";

/// SQLite-backed normalized application records and transactional event outbox.
pub struct SqliteControlStore {
    connection: Mutex<Connection>,
}

impl SqliteControlStore {
    /// Opens or creates a store, applies the schema, and migrates the retired JSON blob.
    ///
    /// # Errors
    ///
    /// Returns an error when the database cannot be opened, initialized, or migrated.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, ControlStoreError> {
        let connection = Connection::open(path).map_err(sql_error)?;
        Self::initialize(connection)
    }

    /// Creates an isolated in-memory store for tests and embedded callers.
    ///
    /// # Errors
    ///
    /// Returns an error when the in-memory database cannot be initialized.
    pub fn in_memory() -> Result<Self, ControlStoreError> {
        Self::initialize(Connection::open_in_memory().map_err(sql_error)?)
    }

    fn initialize(mut connection: Connection) -> Result<Self, ControlStoreError> {
        connection.execute_batch(RECORD_SCHEMA).map_err(sql_error)?;
        migrate_legacy_blob(&mut connection)?;
        Ok(Self {
            connection: Mutex::new(connection),
        })
    }

    /// Creates a transactionally consistent online backup.
    ///
    /// # Errors
    ///
    /// Returns an error when the store cannot be locked or the backup cannot be written.
    pub fn backup_to(&self, destination: impl AsRef<Path>) -> Result<(), ControlStoreError> {
        let connection = self.connection.lock().map_err(lock_error)?;
        connection
            .backup(MAIN_DB, destination, None)
            .map_err(sql_error)
    }

    /// Restores an online backup into this open store.
    ///
    /// # Errors
    ///
    /// Returns an error when the store cannot be locked or the backup cannot be restored.
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
    /// Returns an error when the check cannot run or reports corruption.
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
    async fn read(&self, filters: &[ControlFilter]) -> Result<ControlRead, ControlStoreError> {
        let connection = self.connection.lock().map_err(lock_error)?;
        let revision = revision(&connection)?;
        let mut records = BTreeMap::new();
        for filter in filters {
            read_filter(&connection, filter, &mut records)?;
        }
        Ok(ControlRead {
            revision,
            records: records.into_values().collect(),
        })
    }

    async fn apply(
        &self,
        expected_revision: u64,
        changes: Vec<ControlChange>,
        events: Vec<PendingEvent>,
    ) -> Result<u64, ControlStoreError> {
        let mut connection = self.connection.lock().map_err(lock_error)?;
        let transaction = connection.transaction().map_err(sql_error)?;
        let current = revision(&transaction)?;
        if current != expected_revision {
            return Err(ControlStoreError::Conflict);
        }
        for change in changes {
            apply_change(&transaction, change)?;
        }
        let next = expected_revision.saturating_add(1);
        transaction
            .execute(
                "UPDATE control_metadata SET revision = ?1 WHERE singleton = 1",
                params![next],
            )
            .map_err(sql_error)?;
        append_retained_events(&transaction, events)?;
        transaction.commit().map_err(sql_error)?;
        Ok(next)
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
            .prepare(
                "SELECT run_id, body_json, updated_at FROM run_progress ORDER BY updated_at, run_id",
            )
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

fn table(kind: ControlRecordKind) -> &'static str {
    match kind {
        ControlRecordKind::Project => "projects",
        ControlRecordKind::Agent => "agents",
        ControlRecordKind::Provider => "agent_providers",
        ControlRecordKind::ProviderCredential => "provider_credentials",
        ControlRecordKind::RunCredential => "run_credentials",
        ControlRecordKind::Session => "sessions",
        ControlRecordKind::Message => "messages",
        ControlRecordKind::Run => "runs",
        ControlRecordKind::WorkspaceRunJournal => "workspace_run_journals",
        ControlRecordKind::Cron => "crons",
        ControlRecordKind::Settings => "settings",
    }
}

fn revision(connection: &Connection) -> Result<u64, ControlStoreError> {
    connection
        .query_row(
            "SELECT revision FROM control_metadata WHERE singleton = 1",
            [],
            |row| row.get(0),
        )
        .map_err(sql_error)
}

fn read_filter(
    connection: &Connection,
    filter: &ControlFilter,
    records: &mut BTreeMap<(ControlRecordKind, String), ControlRecord>,
) -> Result<(), ControlStoreError> {
    let (kind, sql, parameter) = match filter {
        ControlFilter::All(kind) => (
            *kind,
            format!("SELECT id, project_id, body_json FROM {}", table(*kind)),
            None,
        ),
        ControlFilter::Id { kind, id } => (
            *kind,
            format!(
                "SELECT id, project_id, body_json FROM {} WHERE id = ?1",
                table(*kind)
            ),
            Some(id.as_str()),
        ),
        ControlFilter::Project { kind, project_id } => (
            *kind,
            format!(
                "SELECT id, project_id, body_json FROM {} WHERE project_id = ?1",
                table(*kind)
            ),
            Some(project_id.as_str()),
        ),
        ControlFilter::MessageAncestors { head_id } => (
            ControlRecordKind::Message,
            "WITH RECURSIVE ancestors(id, project_id, parent_message_id, body_json) AS (
               SELECT id, project_id, parent_message_id, body_json FROM messages WHERE id = ?1
               UNION
               SELECT message.id, message.project_id, message.parent_message_id, message.body_json
                 FROM messages AS message
                 JOIN ancestors ON message.id = ancestors.parent_message_id
             ) SELECT id, project_id, body_json FROM ancestors"
                .into(),
            Some(head_id.as_str()),
        ),
        ControlFilter::RunsForSession { session_id } => (
            ControlRecordKind::Run,
            "SELECT id, project_id, body_json FROM runs
             WHERE json_extract(body_json, '$.session_id') = ?1"
                .into(),
            Some(session_id.as_str()),
        ),
        ControlFilter::RunsForCron { cron_id } => (
            ControlRecordKind::Run,
            "SELECT id, project_id, body_json FROM runs
             WHERE json_extract(body_json, '$.cron_id') = ?1"
                .into(),
            Some(cron_id.as_str()),
        ),
        ControlFilter::AgentsForProvider { provider_id } => (
            ControlRecordKind::Agent,
            "SELECT id, project_id, body_json FROM agents
             WHERE json_extract(body_json, '$.config.provider_id') = ?1"
                .into(),
            Some(provider_id.as_str()),
        ),
    };
    let mut statement = connection.prepare(&sql).map_err(sql_error)?;
    let mut rows = match parameter {
        Some(parameter) => statement.query(params![parameter]),
        None => statement.query([]),
    }
    .map_err(sql_error)?;
    while let Some(row) = rows.next().map_err(sql_error)? {
        let id = row.get::<_, String>(0).map_err(sql_error)?;
        let project_id = row.get::<_, Option<String>>(1).map_err(sql_error)?;
        let body = row.get::<_, String>(2).map_err(sql_error)?;
        records.insert(
            (kind, id.clone()),
            ControlRecord {
                kind,
                id,
                project_id,
                value: serde_json::from_str(&body).map_err(json_error)?,
            },
        );
    }
    Ok(())
}

fn apply_change(
    transaction: &Transaction<'_>,
    change: ControlChange,
) -> Result<(), ControlStoreError> {
    match change {
        ControlChange::Put(record) if record.kind == ControlRecordKind::Message => {
            let body = serde_json::to_string(&record.value).map_err(json_error)?;
            let parent = record
                .value
                .get("parent_message_id")
                .and_then(Value::as_str);
            let inserted = transaction
                .execute(
                    "INSERT OR IGNORE INTO messages(id, project_id, parent_message_id, body_json)
                     VALUES(?1, ?2, ?3, ?4)",
                    params![record.id, record.project_id, parent, body],
                )
                .map_err(sql_error)?;
            if inserted == 0 {
                let existing = transaction
                    .query_row(
                        "SELECT body_json FROM messages WHERE id = ?1",
                        params![record.id],
                        |row| row.get::<_, String>(0),
                    )
                    .map_err(sql_error)?;
                if existing != body {
                    return Err(ControlStoreError::Other(
                        "attempted to modify an immutable Message".into(),
                    ));
                }
            }
            Ok(())
        }
        ControlChange::Put(record) => {
            let table = table(record.kind);
            let sql = format!(
                "INSERT INTO {table}(id, project_id, body_json) VALUES(?1, ?2, ?3)
                 ON CONFLICT(id) DO UPDATE SET project_id = excluded.project_id, body_json = excluded.body_json"
            );
            transaction
                .execute(
                    &sql,
                    params![
                        record.id,
                        record.project_id,
                        serde_json::to_string(&record.value).map_err(json_error)?
                    ],
                )
                .map_err(sql_error)?;
            Ok(())
        }
        ControlChange::Delete { kind, id } => {
            let sql = format!("DELETE FROM {} WHERE id = ?1", table(kind));
            transaction.execute(&sql, params![id]).map_err(sql_error)?;
            Ok(())
        }
    }
}

fn migrate_legacy_blob(connection: &mut Connection) -> Result<(), ControlStoreError> {
    let exists = connection
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'control_state'",
            [],
            |_| Ok(()),
        )
        .optional()
        .map_err(sql_error)?
        .is_some();
    if !exists {
        return Ok(());
    }
    let legacy = connection
        .query_row(
            "SELECT revision, body_json FROM control_state WHERE singleton = 1",
            [],
            |row| Ok((row.get::<_, u64>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()
        .map_err(sql_error)?;
    let transaction = connection.transaction().map_err(sql_error)?;
    if let Some((legacy_revision, body)) = legacy {
        let value: Value = serde_json::from_str(&body).map_err(json_error)?;
        if !value.is_null() {
            // The retired blob predates both record storage and provider-backed
            // Agent configuration. Upgrade the complete snapshot before any row
            // is written, so the split can never leave mixed Agent schemas.
            let value = upgrade_legacy_snapshot(value)?;
            for record in legacy_records(&value)? {
                apply_change(&transaction, ControlChange::Put(record))?;
            }
        }
        transaction
            .execute(
                "UPDATE control_metadata SET revision = MAX(revision, ?1) WHERE singleton = 1",
                params![legacy_revision],
            )
            .map_err(sql_error)?;
    }
    transaction
        .execute("DROP TABLE control_state", [])
        .map_err(sql_error)?;
    transaction.commit().map_err(sql_error)
}

fn upgrade_legacy_snapshot(mut value: Value) -> Result<Value, ControlStoreError> {
    if value.get("providers").is_some() {
        return Ok(value);
    }
    let mut providers = vec![serde_json::json!({
        "id": "builtin-codex",
        "name": "Codex",
        "kind": "codex",
        "url": null,
        "models": [{
            "id": "gpt-5.6-sol",
            "name": "gpt-5.6-sol",
            "reasoning_efforts": ["low", "medium", "high", "xhigh", "max", "ultra"]
        }],
        "has_secret": false
    })];
    if let Some(agents) = value.get_mut("agents").and_then(Value::as_array_mut) {
        for agent in agents {
            let kind = agent.get("mode").and_then(Value::as_str).ok_or_else(|| {
                ControlStoreError::Other("legacy Agent provider kind is missing".into())
            })?;
            let model = agent
                .get("model")
                .and_then(Value::as_str)
                .unwrap_or("default")
                .to_owned();
            let provider = providers
                .iter_mut()
                .find(|provider| provider["kind"] == kind)
                .ok_or_else(|| {
                    ControlStoreError::Other("legacy provider kind is unavailable".into())
                })?;
            let models = provider["models"].as_array_mut().expect("provider models");
            if !models.iter().any(|candidate| candidate["id"] == model) {
                models.push(serde_json::json!({
                    "id": model,
                    "name": model,
                    "reasoning_efforts": []
                }));
            }
            agent["config"] = serde_json::json!({
                "provider_id": provider["id"],
                "model": model,
                "reasoning_effort": null
            });
            let object = agent.as_object_mut().ok_or_else(|| {
                ControlStoreError::Other("legacy Agent record is not an object".into())
            })?;
            object.remove("model");
            object.remove("mode");
        }
    }
    let configs = value["agents"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|agent| {
            Some((
                agent.get("id")?.as_str()?.to_owned(),
                agent.get("config")?.clone(),
            ))
        })
        .collect::<BTreeMap<_, _>>();
    if let Some(runs) = value.get_mut("runs").and_then(Value::as_array_mut) {
        for run in runs {
            let agent_id = run
                .get("agent_id")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let mut config = configs.get(agent_id).cloned().ok_or_else(|| {
                ControlStoreError::Other("legacy Run Agent is unavailable".into())
            })?;
            config["reasoning_effort"] =
                run.get("reasoning_effort").cloned().unwrap_or(Value::Null);
            let provider = providers
                .iter()
                .find(|provider| provider["id"] == config["provider_id"])
                .expect("migrated provider");
            let mut provider = provider.clone();
            provider
                .as_object_mut()
                .expect("provider object")
                .remove("has_secret");
            run["provider"] = provider;
            run["config"] = config;
            run.as_object_mut()
                .ok_or_else(|| {
                    ControlStoreError::Other("legacy Run record is not an object".into())
                })?
                .remove("reasoning_effort");
        }
    }
    value["providers"] = Value::Array(providers);
    Ok(value)
}

fn legacy_records(value: &Value) -> Result<Vec<ControlRecord>, ControlStoreError> {
    let object = value
        .as_object()
        .ok_or_else(|| ControlStoreError::Other("legacy control state is not an object".into()))?;
    let mut records = Vec::new();
    let mut run_projects = BTreeMap::new();
    for (field, kind) in [
        ("projects", ControlRecordKind::Project),
        ("agents", ControlRecordKind::Agent),
        ("providers", ControlRecordKind::Provider),
        ("sessions", ControlRecordKind::Session),
        ("messages", ControlRecordKind::Message),
        ("runs", ControlRecordKind::Run),
        ("crons", ControlRecordKind::Cron),
    ] {
        for item in object
            .get(field)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let id = item
                .get("id")
                .or_else(|| {
                    (kind == ControlRecordKind::Provider)
                        .then(|| item.pointer("/provider/id"))
                        .flatten()
                })
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    ControlStoreError::Other(format!("legacy {field} record has no id"))
                })?
                .to_owned();
            let project_id = if kind == ControlRecordKind::Project {
                Some(id.clone())
            } else {
                item.get("project_id")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            };
            if kind == ControlRecordKind::Run
                && let Some(project_id) = &project_id
            {
                run_projects.insert(id.clone(), project_id.clone());
            }
            records.push(ControlRecord {
                kind,
                id,
                project_id,
                value: item.clone(),
            });
        }
    }
    for (field, kind) in [
        (
            "provider_credentials",
            ControlRecordKind::ProviderCredential,
        ),
        ("run_credentials", ControlRecordKind::RunCredential),
        (
            "workspace_run_journals",
            ControlRecordKind::WorkspaceRunJournal,
        ),
    ] {
        for (id, item) in object
            .get(field)
            .and_then(Value::as_object)
            .into_iter()
            .flatten()
        {
            records.push(ControlRecord {
                kind,
                id: id.clone(),
                project_id: run_projects.get(id).cloned(),
                value: item.clone(),
            });
        }
    }
    if object.contains_key("settings") || object.contains_key("settings_revision") {
        records.push(ControlRecord {
            kind: ControlRecordKind::Settings,
            id: "settings".into(),
            project_id: None,
            value: serde_json::json!({
                "values": object.get("settings").cloned().unwrap_or_else(|| serde_json::json!({})),
                "revision": object.get("settings_revision").cloned().unwrap_or_else(|| serde_json::json!(1)),
            }),
        });
    }
    Ok(records)
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
    use std::sync::Arc;

    use super::*;

    fn record(
        kind: ControlRecordKind,
        id: &str,
        project_id: Option<&str>,
        value: Value,
    ) -> ControlRecord {
        ControlRecord {
            kind,
            id: id.into(),
            project_id: project_id.map(str::to_owned),
            value,
        }
    }

    #[tokio::test]
    async fn reads_are_bounded_by_entity_and_project() {
        let store = SqliteControlStore::in_memory().unwrap();
        store
            .apply(
                0,
                vec![
                    ControlChange::Put(record(
                        ControlRecordKind::Project,
                        "p1",
                        Some("p1"),
                        serde_json::json!({"id":"p1","workdir":"/p1"}),
                    )),
                    ControlChange::Put(record(
                        ControlRecordKind::Project,
                        "p2",
                        Some("p2"),
                        serde_json::json!({"id":"p2","workdir":"/p2"}),
                    )),
                    ControlChange::Put(record(
                        ControlRecordKind::Message,
                        "m1",
                        Some("p1"),
                        serde_json::json!({"id":"m1","project_id":"p1","parent_message_id":null}),
                    )),
                    ControlChange::Put(record(
                        ControlRecordKind::Message,
                        "m2",
                        Some("p2"),
                        serde_json::json!({"id":"m2","project_id":"p2","parent_message_id":null}),
                    )),
                ],
                Vec::new(),
            )
            .await
            .unwrap();

        let read = store
            .read(&[ControlFilter::project(ControlRecordKind::Message, "p1")])
            .await
            .unwrap();
        assert_eq!(read.records.len(), 1);
        assert_eq!(read.records[0].id, "m1");
    }

    #[tokio::test]
    async fn immutable_messages_cannot_be_replaced() {
        let store = SqliteControlStore::in_memory().unwrap();
        store.apply(0, vec![
            ControlChange::Put(record(ControlRecordKind::Project, "p1", Some("p1"), serde_json::json!({"id":"p1","workdir":"/p1"}))),
            ControlChange::Put(record(ControlRecordKind::Message, "m1", Some("p1"), serde_json::json!({"id":"m1","project_id":"p1","parent_message_id":null,"text":"one"}))),
        ], Vec::new()).await.unwrap();
        let failure = store.apply(1, vec![ControlChange::Put(record(
            ControlRecordKind::Message, "m1", Some("p1"),
            serde_json::json!({"id":"m1","project_id":"p1","parent_message_id":null,"text":"two"}),
        ))], Vec::new()).await.unwrap_err();
        assert!(failure.to_string().contains("immutable"));
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn legacy_control_blob_is_split_once_and_removed() {
        let temporary = tempfile::TempDir::new().unwrap();
        let database = temporary.path().join("legacy.sqlite3");
        let legacy = serde_json::json!({
            "projects": [{"id":"p1","workdir":"/p1"}],
            "agents": [
                {"id":"a1","name":"one","mode":"codex","model":"gpt-5.6-sol","owner_session_id":null,"revision":1,"enabled":true},
                {"id":"a2","name":"two","mode":"codex","model":"legacy-model","owner_session_id":null,"revision":1,"enabled":true}
            ],
            "sessions": [],
            "messages": [{"id":"m1","project_id":"p1","parent_message_id":null}],
            "runs": [],
            "crons": [],
            "provider_credentials": {},
            "run_credentials": {},
            "workspace_run_journals": {},
            "settings": {},
            "settings_revision": 3,
        });
        let connection = Connection::open(&database).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE control_state(
                    singleton INTEGER PRIMARY KEY,
                    revision INTEGER NOT NULL,
                    body_json TEXT NOT NULL
                );",
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO control_state(singleton, revision, body_json) VALUES(1, 7, ?1)",
                [legacy.to_string()],
            )
            .unwrap();
        drop(connection);

        let store = SqliteControlStore::open(&database).unwrap();
        let read = store
            .read(&[
                ControlFilter::all(ControlRecordKind::Project),
                ControlFilter::all(ControlRecordKind::Agent),
                ControlFilter::all(ControlRecordKind::Provider),
                ControlFilter::project(ControlRecordKind::Message, "p1"),
                ControlFilter::all(ControlRecordKind::Settings),
            ])
            .await
            .unwrap();
        assert_eq!(read.revision, 7);
        assert_eq!(read.records.len(), 6);
        let agents = read
            .records
            .iter()
            .filter(|record| record.kind == ControlRecordKind::Agent)
            .collect::<Vec<_>>();
        assert_eq!(agents.len(), 2);
        assert!(
            agents
                .iter()
                .all(|agent| agent.value.get("config").is_some())
        );
        assert!(agents.iter().all(|agent| agent.value.get("mode").is_none()));

        let mut updated = agents
            .iter()
            .find(|agent| agent.id == "a1")
            .expect("first migrated Agent")
            .value
            .clone();
        updated["name"] = serde_json::json!("updated");
        store
            .apply(
                read.revision,
                vec![ControlChange::Put(record(
                    ControlRecordKind::Agent,
                    "a1",
                    None,
                    updated,
                ))],
                Vec::new(),
            )
            .await
            .unwrap();
        drop(store);

        let reopened = SqliteControlStore::open(&database).unwrap();
        let agents = reopened
            .read(&[ControlFilter::all(ControlRecordKind::Agent)])
            .await
            .unwrap();
        assert_eq!(agents.records.len(), 2);
        assert!(
            agents
                .records
                .iter()
                .all(|agent| agent.value.get("config").is_some())
        );
        drop(reopened);

        let connection = Connection::open(database).unwrap();
        let legacy_table_count: u64 = connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='control_state'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(legacy_table_count, 0);
    }

    #[tokio::test]
    async fn online_backup_restores_records_and_outbox() {
        let temporary = tempfile::TempDir::new().unwrap();
        let database = temporary.path().join("live.sqlite3");
        let backup = temporary.path().join("backup.sqlite3");
        let store = SqliteControlStore::open(&database).unwrap();
        store
            .apply(
                0,
                vec![ControlChange::Put(record(
                    ControlRecordKind::Project,
                    "p1",
                    Some("p1"),
                    serde_json::json!({"id":"p1","workdir":"/p1"}),
                ))],
                vec![PendingEvent {
                    kind: "test.committed".into(),
                    entity_id: Some("p1".into()),
                    body: serde_json::json!({"revision":1}),
                    created_at: 1,
                }],
            )
            .await
            .unwrap();
        store.backup_to(&backup).unwrap();
        store
            .apply(
                1,
                vec![ControlChange::Put(record(
                    ControlRecordKind::Project,
                    "p2",
                    Some("p2"),
                    serde_json::json!({"id":"p2","workdir":"/p2"}),
                ))],
                Vec::new(),
            )
            .await
            .unwrap();

        store.restore_from(&backup).unwrap();
        store.quick_check().unwrap();
        let recovered = store
            .read(&[ControlFilter::all(ControlRecordKind::Project)])
            .await
            .unwrap();
        assert_eq!(recovered.revision, 1);
        assert_eq!(recovered.records.len(), 1);
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
                body: serde_json::json!({"index":index}),
                created_at: 1,
            })
            .collect();
        store
            .save_progress(
                ProgressCheckpoint {
                    run_id: "run-a".into(),
                    body: serde_json::json!({"seq":RETAINED_EVENTS}),
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
                        body: serde_json::json!({"seq":RETAINED_EVENTS + 64}),
                        updated_at: 2,
                    },
                    (0..64)
                        .map(|index| PendingEvent {
                            kind: "run.progress".into(),
                            entity_id: Some("run-a".into()),
                            body: serde_json::json!({"new":index}),
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
            assert_eq!(page.events.first().unwrap().cursor, 2);
        } else {
            assert!(page.events.is_empty());
            assert!(page.bounds.oldest.is_some_and(|oldest| oldest > 2));
        }
    }
}
