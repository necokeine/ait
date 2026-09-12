//! Global catalog plus independently portable Project histories. A durable commit
//! decision and prepared Project batches bridge files without a cross-WAL transaction.

use std::{fs::File, path::PathBuf};

use serde::{Deserialize, Serialize};

use super::{
    BTreeMap, Connection, ControlChange, ControlFilter, ControlRead, ControlRecordKind,
    ControlStore, ControlStoreError, DurableEvent, DurableEventPage, EventBounds, MAIN_DB, Mutex,
    OptionalExtension, Path, PendingEvent, ProgressCheckpoint, RECORD_SCHEMA, RETAINED_EVENTS,
    Transaction, Value, apply_change, async_trait, event_bounds_locked, json_error, lock_error,
    migrate_legacy_blob, params, read_filter, replay_locked, revision, sql_error, table,
};

mod migration;
mod project;
use project::{finish_project, open_project, prepare_project};

const GLOBAL_APPLICATION_ID: u32 = 0x4149_4731; // AIG1
const PROJECT_KINDS: [ControlRecordKind; 5] = [
    ControlRecordKind::Message,
    ControlRecordKind::Session,
    ControlRecordKind::Run,
    ControlRecordKind::RunCredential,
    ControlRecordKind::WorkspaceRunJournal,
];

/// File-backed catalog with each Project's history in `.metafab/project.sqlite3`.
///
/// All cooperating processes serialize through a catalog lock. Project payloads
/// (including prepared changes and event bodies) never enter the global journal.
pub struct SplitSqliteControlStore {
    connection: Mutex<Connection>,
    lock_path: PathBuf,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct ProjectTarget {
    id: String,
    workdir: String,
}

#[derive(Default, Serialize, Deserialize)]
struct ProjectBatch {
    changes: Vec<ControlChange>,
    events: Vec<DurableEvent>,
    progress: Vec<ProgressCheckpoint>,
    clear_progress: Vec<String>,
}

#[derive(Serialize, Deserialize)]
struct LocationChange {
    kind: ControlRecordKind,
    id: String,
    project_id: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct RoutedEvent {
    event: DurableEvent,
    project_id: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct Commit {
    id: String,
    revision: u64,
    targets: BTreeMap<String, ProjectTarget>,
    changes: Vec<ControlChange>,
    locations: Vec<LocationChange>,
    events: Vec<RoutedEvent>,
    migration: bool,
}

impl SplitSqliteControlStore {
    /// Opens the global catalog and recovers any previously decided commit.
    /// Existing single-file databases are backed up and migrated before use.
    ///
    /// # Errors
    /// Returns an error for unsupported formats, unavailable migration targets,
    /// identity mismatches, or filesystem/SQLite failures.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, ControlStoreError> {
        let connection = Connection::open(path).map_err(sql_error)?;
        let path = std::fs::canonicalize(
            connection
                .path()
                .ok_or_else(|| other("global database needs a filesystem path"))?,
        )
        .map_err(io_error)?;
        let mut lock_path = path.as_os_str().to_os_string();
        lock_path.push(".lock");
        let store = Self {
            connection: Mutex::new(connection),
            lock_path: lock_path.into(),
        };
        let _lock = store.lock_file()?;
        let mut connection = store.connection.lock().map_err(lock_error)?;
        connection
            .busy_timeout(std::time::Duration::from_secs(5))
            .map_err(sql_error)?;
        migration::initialize(&mut connection, &path)?;
        recover(&mut connection)?;
        drop(connection);
        Ok(store)
    }

    fn lock_file(&self) -> Result<File, ControlStoreError> {
        let file = File::options()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&self.lock_path)
            .map_err(io_error)?;
        file.lock().map_err(io_error)?;
        Ok(file)
    }

    fn access<T>(
        &self,
        action: impl FnOnce(&mut Connection) -> Result<T, ControlStoreError>,
    ) -> Result<T, ControlStoreError> {
        let mut connection = self.connection.lock().map_err(lock_error)?;
        let _lock = self.lock_file()?;
        recover(&mut connection)?;
        action(&mut connection)
    }

    /// Backs up the global catalog only; Project histories need separate backups.
    ///
    /// # Errors
    /// Returns a persistence or backup error.
    pub fn backup_global_to(&self, destination: impl AsRef<Path>) -> Result<(), ControlStoreError> {
        self.access(|connection| {
            connection
                .backup(MAIN_DB, destination, None)
                .map_err(sql_error)
        })
    }

    /// Creates a standalone online backup of one Project, including its identity.
    ///
    /// # Errors
    /// Returns an error if the Project is unavailable or the backup fails.
    pub fn backup_project_to(
        &self,
        project_id: &str,
        destination: impl AsRef<Path>,
    ) -> Result<(), ControlStoreError> {
        self.access(|connection| {
            let target = target(connection, project_id)?
                .ok_or_else(|| other("Project is not registered"))?;
            open_project(&target, &owner(connection)?, false)?
                .backup(MAIN_DB, destination, None)
                .map_err(sql_error)
        })
    }
}

#[async_trait]
impl ControlStore for SplitSqliteControlStore {
    async fn read(&self, filters: &[ControlFilter]) -> Result<ControlRead, ControlStoreError> {
        self.access(|connection| {
            let mut records = BTreeMap::new();
            let mut projects_open = BTreeMap::new();
            let owner = owner(connection)?;
            for filter in filters {
                match filter_projects(connection, filter)? {
                    None => read_filter(connection, filter, &mut records)?,
                    Some(projects) => {
                        for project_id in projects {
                            if target(connection, &project_id)?.is_some() {
                                let project = cached_project(
                                    connection,
                                    &mut projects_open,
                                    &owner,
                                    &project_id,
                                )?;
                                read_filter(project, filter, &mut records)?;
                            }
                        }
                    }
                }
            }
            Ok(ControlRead {
                revision: revision(connection)?,
                records: records.into_values().collect(),
            })
        })
    }

    async fn apply(
        &self,
        expected_revision: u64,
        changes: Vec<ControlChange>,
        events: Vec<PendingEvent>,
    ) -> Result<u64, ControlStoreError> {
        self.access(|connection| {
            if revision(connection)? != expected_revision {
                return Err(ControlStoreError::Conflict);
            }
            let next = expected_revision
                .checked_add(1)
                .ok_or_else(|| other("control revision exhausted"))?;
            let (mut commit, mut batches) = build_commit(connection, next, changes)?;
            route_events(connection, &mut commit, &mut batches, events)?;
            submit(connection, &commit, &batches)?;
            Ok(next)
        })
    }

    async fn replay(
        &self,
        cursor: u64,
        limit: usize,
    ) -> Result<Vec<DurableEvent>, ControlStoreError> {
        self.access(|connection| replay_split(connection, cursor, limit))
    }

    async fn event_bounds(&self) -> Result<EventBounds, ControlStoreError> {
        self.access(|connection| event_bounds_locked(connection))
    }

    async fn replay_page(
        &self,
        cursor: u64,
        limit: usize,
    ) -> Result<DurableEventPage, ControlStoreError> {
        self.access(|connection| {
            let bounds = event_bounds_locked(connection)?;
            let cursor_valid = cursor == 0
                || bounds
                    .oldest
                    .is_some_and(|oldest| cursor >= oldest.saturating_sub(1))
                    && bounds.latest.is_some_and(|latest| cursor <= latest);
            let events = if cursor_valid {
                replay_split(connection, cursor, limit)?
            } else {
                Vec::new()
            };
            Ok(DurableEventPage {
                bounds,
                events,
                cursor_valid,
            })
        })
    }

    async fn save_progress(
        &self,
        checkpoint: ProgressCheckpoint,
        events: Vec<PendingEvent>,
    ) -> Result<(), ControlStoreError> {
        self.access(|connection| {
            let project_id = location(connection, ControlRecordKind::Run, &checkpoint.run_id)?
                .ok_or_else(|| other("progress Run does not exist"))?;
            if checkpoint.body.get("project_id").and_then(Value::as_str) != Some(&project_id) {
                return Err(other("progress Project does not match Run"));
            }
            let (mut commit, mut batches) =
                build_commit(connection, revision(connection)?, Vec::new())?;
            ensure_target(connection, &mut commit, &project_id)?;
            batches
                .entry(project_id)
                .or_default()
                .progress
                .push(checkpoint);
            route_events(connection, &mut commit, &mut batches, events)?;
            submit(connection, &commit, &batches)
        })
    }

    async fn load_progress(
        &self,
        project_id: &str,
    ) -> Result<Vec<ProgressCheckpoint>, ControlStoreError> {
        self.access(|connection| {
            let Some(target) = target(connection, project_id)? else {
                return Ok(Vec::new());
            };
            let project = open_project(&target, &owner(connection)?, false)?;
            progress_rows(&project)
        })
    }

    async fn clear_progress(&self, run_id: &str) -> Result<(), ControlStoreError> {
        self.access(|connection| {
            let Some(project_id) = location(connection, ControlRecordKind::Run, run_id)? else {
                return Ok(());
            };
            let (mut commit, mut batches) =
                build_commit(connection, revision(connection)?, Vec::new())?;
            ensure_target(connection, &mut commit, &project_id)?;
            batches
                .entry(project_id)
                .or_default()
                .clear_progress
                .push(run_id.into());
            submit(connection, &commit, &batches)
        })
    }
}

fn is_project(kind: ControlRecordKind) -> bool {
    PROJECT_KINDS.contains(&kind)
}

fn location(
    connection: &Connection,
    kind: ControlRecordKind,
    id: &str,
) -> Result<Option<String>, ControlStoreError> {
    connection
        .query_row(
            "SELECT project_id FROM record_locations WHERE kind=?1 AND id=?2",
            params![table(kind), id],
            |row| row.get(0),
        )
        .optional()
        .map_err(sql_error)
}

fn target(connection: &Connection, id: &str) -> Result<Option<ProjectTarget>, ControlStoreError> {
    connection
        .query_row(
            "SELECT json_extract(body_json, '$.workdir') FROM projects WHERE id=?1",
            [id],
            |row| {
                Ok(ProjectTarget {
                    id: id.into(),
                    workdir: row.get(0)?,
                })
            },
        )
        .optional()
        .map_err(sql_error)
}

fn owner(connection: &Connection) -> Result<String, ControlStoreError> {
    connection
        .query_row(
            "SELECT coordinator_id FROM split_metadata WHERE singleton=1",
            [],
            |row| row.get(0),
        )
        .map_err(sql_error)
}

fn filter_projects(
    connection: &Connection,
    filter: &ControlFilter,
) -> Result<Option<Vec<String>>, ControlStoreError> {
    let selection = match filter {
        ControlFilter::All(kind) if is_project(*kind) => {
            let mut statement = connection.prepare("SELECT DISTINCT project_id FROM record_locations WHERE kind=?1 ORDER BY project_id").map_err(sql_error)?;
            let rows = statement
                .query_map([table(*kind)], |row| row.get(0))
                .map_err(sql_error)?;
            return Ok(Some(
                rows.collect::<Result<Vec<_>, _>>().map_err(sql_error)?,
            ));
        }
        ControlFilter::Project { kind, project_id } if is_project(*kind) => {
            return Ok(Some(vec![project_id.clone()]));
        }
        ControlFilter::Id { kind, id } if is_project(*kind) => location(connection, *kind, id)?,
        ControlFilter::MessageAncestors { head_id } => {
            location(connection, ControlRecordKind::Message, head_id)?
        }
        ControlFilter::MessageChildren { parent_id } => {
            location(connection, ControlRecordKind::Message, parent_id)?
        }
        ControlFilter::RunsForSession { session_id } => {
            location(connection, ControlRecordKind::Session, session_id)?
        }
        ControlFilter::RunsForCron { cron_id } => connection
            .query_row(
                "SELECT project_id FROM crons WHERE id=?1",
                [cron_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(sql_error)?,
        _ => return Ok(None),
    };
    Ok(Some(selection.into_iter().collect()))
}

fn ensure_target(
    connection: &Connection,
    commit: &mut Commit,
    project_id: &str,
) -> Result<(), ControlStoreError> {
    if !commit.targets.contains_key(project_id) {
        commit.targets.insert(
            project_id.into(),
            target(connection, project_id)?
                .ok_or_else(|| other(format!("Project {project_id} is not registered")))?,
        );
    }
    Ok(())
}

fn build_commit(
    connection: &Connection,
    next: u64,
    changes: Vec<ControlChange>,
) -> Result<(Commit, BTreeMap<String, ProjectBatch>), ControlStoreError> {
    let mut commit = Commit {
        id: connection
            .query_row("SELECT lower(hex(randomblob(16)))", [], |row| row.get(0))
            .map_err(sql_error)?,
        revision: next,
        targets: BTreeMap::new(),
        changes: Vec::new(),
        locations: Vec::new(),
        events: Vec::new(),
        migration: false,
    };
    let mut batches = BTreeMap::<String, ProjectBatch>::new();
    for change in &changes {
        if let ControlChange::Put(record) = change
            && record.kind == ControlRecordKind::Project
        {
            let workdir = record
                .value
                .get("workdir")
                .and_then(Value::as_str)
                .ok_or_else(|| other("Project workdir is missing"))?
                .to_owned();
            if let Some(existing) = target(connection, &record.id)? {
                if existing.workdir != workdir {
                    return Err(other(
                        "Project relocation requires an explicit import/rebind operation",
                    ));
                }
                // Catalog-only edits remain available while a Project is offline.
                continue;
            }
            commit.targets.insert(
                record.id.clone(),
                ProjectTarget {
                    id: record.id.clone(),
                    workdir,
                },
            );
            batches.entry(record.id.clone()).or_default();
        }
    }
    for change in changes {
        let (kind, id) = match &change {
            ControlChange::Put(record) => (record.kind, record.id.as_str()),
            ControlChange::Delete { kind, id } => (*kind, id.as_str()),
        };
        if !is_project(kind) {
            if matches!(
                &change,
                ControlChange::Delete {
                    kind: ControlRecordKind::Project,
                    ..
                }
            ) {
                return Err(other(
                    "Project registry deletion requires an explicit unregister operation",
                ));
            }
            commit.changes.push(change);
            continue;
        }
        let existing = location(connection, kind, id)?;
        let project_id = match &change {
            ControlChange::Put(record) => {
                let project_id = record
                    .project_id
                    .clone()
                    .ok_or_else(|| other("Project record is missing project_id"))?;
                if existing
                    .as_ref()
                    .is_some_and(|existing| *existing != project_id)
                    || record
                        .value
                        .get("project_id")
                        .and_then(Value::as_str)
                        .is_some_and(|body_id| body_id != project_id)
                {
                    return Err(other("record Project identity cannot change"));
                }
                project_id
            }
            ControlChange::Delete { .. } => {
                let Some(existing) = existing else {
                    continue;
                };
                existing
            }
        };
        ensure_target(connection, &mut commit, &project_id)?;
        commit.locations.push(LocationChange {
            kind,
            id: id.into(),
            project_id: matches!(&change, ControlChange::Put(_)).then(|| project_id.clone()),
        });
        batches.entry(project_id).or_default().changes.push(change);
    }
    Ok((commit, batches))
}

fn route_events(
    connection: &Connection,
    commit: &mut Commit,
    batches: &mut BTreeMap<String, ProjectBatch>,
    events: Vec<PendingEvent>,
) -> Result<(), ControlStoreError> {
    let latest: u64 = connection
        .query_row(
            "SELECT COALESCE((SELECT seq FROM sqlite_sequence WHERE name='durable_events'), 0)",
            [],
            |row| row.get(0),
        )
        .map_err(sql_error)?;
    for (index, event) in events.into_iter().enumerate() {
        let cursor = latest
            .checked_add(index as u64 + 1)
            .ok_or_else(|| other("event cursor exhausted"))?;
        route_event(
            connection,
            commit,
            batches,
            DurableEvent {
                cursor,
                kind: event.kind,
                entity_id: event.entity_id,
                body: event.body,
                created_at: event.created_at,
            },
        )?;
    }
    Ok(())
}

fn route_event(
    connection: &Connection,
    commit: &mut Commit,
    batches: &mut BTreeMap<String, ProjectBatch>,
    mut event: DurableEvent,
) -> Result<(), ControlStoreError> {
    let project_id = event
        .body
        .get("project_id")
        .and_then(Value::as_str)
        .map(str::to_owned);
    // Project catalog events are global; Run-triggered Cron events contain Run history.
    let project_id = project_id.filter(|_| {
        !(event.kind.starts_with("project.")
            || event.kind.starts_with("cron.") && event.kind != "cron.run_triggered")
    });
    if let Some(project_id) = &project_id {
        ensure_target(connection, commit, project_id)?;
        batches
            .entry(project_id.clone())
            .or_default()
            .events
            .push(event.clone());
        event.body = Value::Null;
    } else if event.kind.starts_with("run.")
        || event.kind.starts_with("session.")
        || event.kind.starts_with("message.")
    {
        return Err(other("Project event is missing project_id"));
    }
    commit.events.push(RoutedEvent { event, project_id });
    Ok(())
}

fn submit(
    connection: &mut Connection,
    commit: &Commit,
    batches: &BTreeMap<String, ProjectBatch>,
) -> Result<(), ControlStoreError> {
    let validation = connection.transaction().map_err(sql_error)?;
    for change in &commit.changes {
        apply_change(&validation, change.clone())?;
    }
    check_foreign_keys(&validation)?;
    validation.rollback().map_err(sql_error)?;
    let owner = owner(connection)?;
    for (id, batch) in batches {
        let create = commit.migration || target(connection, id)?.is_none();
        let mut project = open_project(&commit.targets[id], &owner, create)?;
        prepare_project(&mut project, &commit.id, batch)?;
    }
    connection
        .execute(
            "INSERT INTO pending_commit VALUES(1, ?1)",
            [serde_json::to_string(commit).map_err(json_error)?],
        )
        .map_err(sql_error)?;
    recover(connection)
}

fn recover(connection: &mut Connection) -> Result<(), ControlStoreError> {
    let body: Option<String> = connection
        .query_row(
            "SELECT body_json FROM pending_commit WHERE singleton=1",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(sql_error)?;
    let Some(body) = body else {
        return Ok(());
    };
    let commit: Commit = serde_json::from_str(&body).map_err(json_error)?;
    let owner = owner(connection)?;
    for target in commit.targets.values() {
        finish_project(&mut open_project(target, &owner, false)?, &commit.id)?;
    }
    let transaction = connection.transaction().map_err(sql_error)?;
    for change in commit.changes {
        apply_change(&transaction, change)?;
    }
    for location in commit.locations {
        if let Some(project_id) = location.project_id {
            transaction.execute("INSERT INTO record_locations VALUES(?1, ?2, ?3) ON CONFLICT(kind,id) DO UPDATE SET project_id=excluded.project_id", params![table(location.kind), location.id, project_id]).map_err(sql_error)?;
        } else {
            transaction
                .execute(
                    "DELETE FROM record_locations WHERE kind=?1 AND id=?2",
                    params![table(location.kind), location.id],
                )
                .map_err(sql_error)?;
        }
    }
    for routed in commit.events {
        write_global_event(&transaction, &routed.event, routed.project_id.as_deref())?;
    }
    transaction
        .execute(
            "UPDATE control_metadata SET revision=?1 WHERE singleton=1",
            [commit.revision],
        )
        .map_err(sql_error)?;
    transaction.execute("DELETE FROM durable_events WHERE cursor < COALESCE((SELECT cursor FROM durable_events ORDER BY cursor DESC LIMIT 1 OFFSET ?1),0)", [RETAINED_EVENTS - 1]).map_err(sql_error)?;
    if commit.migration {
        migration::finish(&transaction)?;
    }
    transaction
        .execute("DELETE FROM pending_commit", [])
        .map_err(sql_error)?;
    transaction.commit().map_err(sql_error)
}

fn write_global_event(
    connection: &Connection,
    event: &DurableEvent,
    project_id: Option<&str>,
) -> Result<(), ControlStoreError> {
    connection.execute("INSERT INTO durable_events(cursor,kind,entity_id,body_json,created_at,project_id) VALUES(?1,?2,?3,?4,?5,?6) ON CONFLICT(cursor) DO UPDATE SET body_json=excluded.body_json, project_id=excluded.project_id", params![event.cursor,event.kind,event.entity_id,event.body.to_string(),event.created_at,project_id]).map_err(sql_error)?;
    Ok(())
}

fn replay_split(
    connection: &Connection,
    cursor: u64,
    limit: usize,
) -> Result<Vec<DurableEvent>, ControlStoreError> {
    let mut events = replay_locked(connection, cursor, limit)?;
    let mut projects_open = BTreeMap::new();
    let owner = owner(connection)?;
    for event in &mut events {
        let project_id: Option<String> = connection
            .query_row(
                "SELECT project_id FROM durable_events WHERE cursor=?1",
                [event.cursor],
                |row| row.get(0),
            )
            .map_err(sql_error)?;
        if let Some(project_id) = project_id {
            let project = cached_project(connection, &mut projects_open, &owner, &project_id)?;
            let body: String = project
                .query_row(
                    "SELECT body_json FROM durable_events WHERE cursor=?1",
                    [event.cursor],
                    |row| row.get(0),
                )
                .map_err(sql_error)?;
            event.body = serde_json::from_str(&body).map_err(json_error)?;
        }
    }
    Ok(events)
}

fn cached_project<'a>(
    connection: &Connection,
    cache: &'a mut BTreeMap<String, Connection>,
    owner: &str,
    project_id: &str,
) -> Result<&'a Connection, ControlStoreError> {
    use std::collections::btree_map::Entry;
    match cache.entry(project_id.into()) {
        Entry::Occupied(entry) => Ok(entry.into_mut()),
        Entry::Vacant(entry) => {
            let target = target(connection, project_id)?
                .ok_or_else(|| other("Project is not registered"))?;
            Ok(entry.insert(open_project(&target, owner, false)?))
        }
    }
}

fn progress_rows(connection: &Connection) -> Result<Vec<ProgressCheckpoint>, ControlStoreError> {
    let mut statement = connection
        .prepare(
            "SELECT run_id, body_json, updated_at FROM run_progress ORDER BY updated_at,run_id",
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

fn check_foreign_keys(connection: &Connection) -> Result<(), ControlStoreError> {
    let violation = connection
        .query_row("PRAGMA foreign_key_check", [], |row| {
            row.get::<_, String>(0)
        })
        .optional()
        .map_err(sql_error)?;
    if let Some(table) = violation {
        return Err(other(format!("foreign key violation in {table}")));
    }
    Ok(())
}

fn pragma_number(connection: &Connection, name: &str) -> Result<u32, ControlStoreError> {
    connection
        .pragma_query_value(None, name, |row| row.get(0))
        .map_err(sql_error)
}

fn table_count(connection: &Connection) -> Result<u64, ControlStoreError> {
    connection
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'",
            [],
            |row| row.get(0),
        )
        .map_err(sql_error)
}

fn other(message: impl Into<String>) -> ControlStoreError {
    ControlStoreError::Other(message.into())
}
#[allow(clippy::needless_pass_by_value)]
fn io_error(error: std::io::Error) -> ControlStoreError {
    other(error.to_string())
}

#[cfg(test)]
mod tests;
