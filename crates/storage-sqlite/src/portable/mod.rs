//! Project-local transactions and runtime ownership, with a rebuildable catalog.
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    path::{Path, PathBuf},
    sync::Mutex,
};

use ait_ports::{
    ControlChange, ControlFilter, ControlRead, ControlRecord, ControlRecordKind as Kind,
    ControlStore, ControlStoreError, ControlVersion, DurableEvent, DurableEventPage, EventBounds,
    PendingEvent, ProgressCheckpoint, ProjectVersion,
};
use async_trait::async_trait;
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::Value;

use super::{
    apply_change, event_bounds_locked, json_error, lock_error, read_filter, replay_locked,
    revision, sql_error, table,
};

mod binding;
mod conversion;
mod native_bindings;
mod ownership;
mod project;
mod projection;
mod read;
mod workers;
mod write;

const GLOBAL_ID: u32 = 0x4149_4731;
const PROJECT_ID: u32 = 0x4149_5031;
const FORMAT: u32 = 3;

struct OwnedProject {
    connection: Connection,
    root: PathBuf,
    identity: ownership::FileIdentity,
    _locks: ownership::ProjectLocks,
}

struct State {
    catalog: Connection,
    projects: BTreeMap<String, OwnedProject>,
    closed: BTreeSet<String>,
    draining: BTreeSet<String>,
}

/// Catalog configuration and independently recoverable, lifetime-locked Projects.
pub struct PortableSqliteControlStore {
    state: Mutex<State>,
    lock_path: PathBuf,
    runtime_instance_id: String,
}

impl PortableSqliteControlStore {
    /// Converts a stopped format-2 catalog and its Projects, preserving identities and history.
    /// Run only after stopping every old daemon and worker that uses this catalog.
    /// Repeating the call resumes an interrupted conversion; backups are retained.
    ///
    /// # Errors
    /// Rejects unsupported formats, unavailable Projects, busy locks and identity mismatches.
    pub fn upgrade_storage(path: impl AsRef<Path>) -> Result<(), ControlStoreError> {
        conversion::upgrade(path.as_ref())
    }
    /// Opens a format-3 catalog. Legacy catalogs require an explicit offline conversion.
    ///
    /// # Errors
    /// Rejects incompatible formats, inaccessible paths and invalid SQLite state.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, ControlStoreError> {
        let mut catalog = Connection::open(path).map_err(sql_error)?;
        catalog
            .busy_timeout(std::time::Duration::from_secs(5))
            .map_err(sql_error)?;
        let path = std::fs::canonicalize(
            catalog
                .path()
                .ok_or_else(|| other("catalog path missing"))?,
        )
        .map_err(io_error)?;
        let lock_path = path.with_file_name(format!(
            "{}.lock",
            path.file_name()
                .ok_or_else(|| other("catalog filename missing"))?
                .to_string_lossy()
        ));
        let _lock = catalog_lock(&lock_path)?;
        let format: u32 = catalog
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .map_err(sql_error)?;
        let application: u32 = catalog
            .pragma_query_value(None, "application_id", |row| row.get(0))
            .map_err(sql_error)?;
        let tables: u64 = catalog.query_row("SELECT count(*) FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'", [], |row| row.get(0)).map_err(sql_error)?;
        if format == 0 && application == 0 && tables == 0 {
            catalog
                .execute_batch(
                    "PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; PRAGMA foreign_keys=ON;",
                )
                .map_err(sql_error)?;
            let transaction = catalog.transaction().map_err(sql_error)?;
            let schema = include_str!("../split/global.sql")
                .splitn(4, ';')
                .nth(3)
                .ok_or_else(|| other("bundled catalog schema is invalid"))?;
            transaction.execute_batch(schema).map_err(sql_error)?;
            transaction
                .execute_batch(include_str!("catalog.sql"))
                .map_err(sql_error)?;
            transaction
                .pragma_update(None, "application_id", GLOBAL_ID)
                .map_err(sql_error)?;
            transaction
                .pragma_update(None, "user_version", FORMAT)
                .map_err(sql_error)?;
            transaction.commit().map_err(sql_error)?;
        } else if application != GLOBAL_ID || format != FORMAT {
            return Err(other(
                "LEGACY_RECOVERY_REQUIRED: stop old Ait backends and workers, then run ait-daemon --database <original-catalog-path> --upgrade-storage before opening this catalog",
            ));
        }
        let incomplete: bool = catalog
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM conversion_manifest WHERE phase!='converted')",
                [],
                |row| row.get(0),
            )
            .map_err(sql_error)?;
        if incomplete {
            return Err(other(
                "LEGACY_RECOVERY_REQUIRED: storage conversion was interrupted; rerun --upgrade-storage with the original catalog",
            ));
        }
        catalog
            .execute_batch("PRAGMA foreign_keys=ON; PRAGMA synchronous=FULL;")
            .map_err(sql_error)?;
        let runtime_instance_id = random_id(&catalog)?;
        Ok(Self {
            state: Mutex::new(State {
                catalog,
                projects: BTreeMap::new(),
                closed: BTreeSet::new(),
                draining: BTreeSet::new(),
            }),
            lock_path,
            runtime_instance_id,
        })
    }

    fn access<T>(
        &self,
        action: impl FnOnce(&mut State) -> Result<T, ControlStoreError>,
    ) -> Result<T, ControlStoreError> {
        let mut state = self.state.lock().map_err(lock_error)?;
        let _lock = catalog_lock(&self.lock_path)?;
        action(&mut state)
    }
}

#[async_trait]
impl ControlStore for PortableSqliteControlStore {
    async fn register_worker_process(
        &self,
        owner: &ait_domain::ProjectOwner,
        pid: u32,
    ) -> Result<(), ControlStoreError> {
        self.access(|state| workers::register(state, owner, pid))
    }
    async fn release_worker_process(
        &self,
        owner: &ait_domain::ProjectOwner,
        pid: u32,
    ) -> Result<(), ControlStoreError> {
        self.access(|state| workers::release(state, owner, pid))
    }
    async fn begin_project_drain(&self, id: &str) -> Result<(), ControlStoreError> {
        self.access(|state| {
            if !state.projects.contains_key(id) {
                return Err(ControlStoreError::Conflict);
            }
            state.draining.insert(id.to_owned());
            Ok(())
        })
    }

    async fn project_can_recover(&self, id: &str) -> Result<bool, ControlStoreError> {
        self.access(|state| {
            let owned = state.projects.get(id).ok_or(ControlStoreError::Conflict)?;
            let origin: String = owned
                .connection
                .query_row(
                    "SELECT origin_catalog FROM project_identity WHERE singleton=1",
                    [],
                    |row| row.get(0),
                )
                .map_err(sql_error)?;
            Ok(origin == catalog_id(&state.catalog)?
                && workers::assert_prior_quiescent(&owned.connection).is_ok())
        })
    }

    async fn release_idle_project(&self, id: &str) -> Result<(), ControlStoreError> {
        self.access(|state| {
            if let Some(owned) = state.projects.get(id) {
                if workers::assert_quiescent(&owned.connection).is_err() {
                    return Ok(());
                }
                owned
                    .connection
                    .execute(
                        "UPDATE project_identity SET owner_instance='' WHERE singleton=1",
                        [],
                    )
                    .map_err(sql_error)?;
            }
            state.projects.remove(id);
            Ok(())
        })
    }

    async fn project_is_open(&self, id: &str) -> Result<bool, ControlStoreError> {
        self.access(|state| Ok(state.projects.contains_key(id)))
    }

    async fn bind_project_agent(
        &self,
        version: &ControlVersion,
        project_id: &str,
        source_agent_id: &str,
        agent_id: &str,
    ) -> Result<(), ControlStoreError> {
        self.access(|state| binding::bind(state, version, project_id, source_agent_id, agent_id))
    }
    async fn open_project(
        &self,
        workdir: &str,
    ) -> Result<Option<ControlRecord>, ControlStoreError> {
        self.access(|state| {
            let root = std::fs::canonicalize(workdir).map_err(io_error)?;
            let Some(id) = project::identity(&root)? else {
                return Ok(None);
            };
            project::mount(state, &id, &root, &self.runtime_instance_id)?;
            let owned = &state.projects[&id];
            let record = project::record(owned, &id)?;
            projection::refresh(&mut state.catalog, &id, owned)?;
            Ok(Some(record))
        })
    }

    async fn close_project(&self, id: &str) -> Result<(), ControlStoreError> {
        self.access(|state| {
            if let Some(owned) = state.projects.get(id) {
                workers::assert_quiescent(&owned.connection)?;
                let active: bool = owned.connection.query_row("SELECT EXISTS(SELECT 1 FROM sessions WHERE json_extract(body_json,'$.active_run_id') IS NOT NULL)",[],|row|row.get(0)).map_err(sql_error)?;
                if active { return Err(other("PROJECT_BUSY: cancel and drain active Runs before closing the Project")); }
                owned.connection.execute("UPDATE project_identity SET owner_instance='' WHERE singleton=1 AND owner_instance=?1",[&self.runtime_instance_id]).map_err(sql_error)?;
            }
            state.projects.remove(id);
            state.closed.insert(id.to_owned());
            state.draining.remove(id);
            Ok(())
        })
    }

    async fn event_namespace(&self) -> Result<String, ControlStoreError> {
        self.access(|state| state.catalog.query_row("SELECT catalog_id || ':' || feed_generation FROM portable_catalog WHERE singleton=1",[],|row|row.get(0)).map_err(sql_error))
    }

    async fn read(&self, filters: &[ControlFilter]) -> Result<ControlRead, ControlStoreError> {
        self.access(|state| read::records(state, filters, &self.runtime_instance_id))
    }

    async fn apply(
        &self,
        _expected_revision: u64,
        _changes: Vec<ControlChange>,
        _events: Vec<PendingEvent>,
    ) -> Result<u64, ControlStoreError> {
        Err(other(
            "Project-aware storage requires an observed ControlVersion",
        ))
    }

    async fn apply_versioned(
        &self,
        version: &ControlVersion,
        changes: Vec<ControlChange>,
        events: Vec<PendingEvent>,
    ) -> Result<u64, ControlStoreError> {
        self.access(|state| {
            write::commit(state, &self.runtime_instance_id, version, changes, &events)
        })
    }

    async fn replay(
        &self,
        cursor: u64,
        limit: usize,
    ) -> Result<Vec<DurableEvent>, ControlStoreError> {
        self.access(|state| {
            for (id, owned) in &state.projects {
                let _projection = projection::refresh(&mut state.catalog, id, owned);
            }
            projection::replay(state, cursor, limit)
        })
    }

    async fn event_bounds(&self) -> Result<EventBounds, ControlStoreError> {
        self.access(|state| event_bounds_locked(&state.catalog))
    }

    async fn replay_page(
        &self,
        cursor: u64,
        limit: usize,
    ) -> Result<DurableEventPage, ControlStoreError> {
        self.access(|state| {
            for (id, owned) in &state.projects {
                let _projection = projection::refresh(&mut state.catalog, id, owned);
            }
            let bounds = event_bounds_locked(&state.catalog)?;
            let cursor_valid = cursor == 0
                || bounds
                    .oldest
                    .is_some_and(|oldest| cursor >= oldest.saturating_sub(1))
                    && bounds.latest.is_some_and(|latest| cursor <= latest);
            let events = if cursor_valid {
                projection::replay(state, cursor, limit)?
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
        self.access(|state| {
            let id = read::location(state,Kind::Run,&checkpoint.run_id)?.ok_or_else(||other("progress Run does not exist"))?;
            if checkpoint.body.get("project_id").and_then(Value::as_str)!=Some(&id) { return Err(other("progress Project does not match Run")); }
            let owned = state.projects.get_mut(&id).ok_or_else(||other("Project is not open for progress publication"))?;
            workers::assert_prior_quiescent(&owned.connection)?;
            let transaction = owned.connection.transaction().map_err(sql_error)?;
            transaction.execute("INSERT INTO run_progress VALUES(?1,?2,?3) ON CONFLICT(run_id) DO UPDATE SET body_json=excluded.body_json,updated_at=excluded.updated_at",params![checkpoint.run_id,serde_json::to_string(&checkpoint.body).map_err(json_error)?,checkpoint.updated_at]).map_err(sql_error)?;
            write::append_events(&transaction,&events,false)?;
            transaction.commit().map_err(sql_error)?;
            let _projection = projection::refresh(&mut state.catalog,&id,owned);
            Ok(())
        })
    }

    async fn load_progress(&self, id: &str) -> Result<Vec<ProgressCheckpoint>, ControlStoreError> {
        self.access(|state| {
            project::acquire(state,id,&self.runtime_instance_id)?;
            let mut statement = state.projects[id].connection.prepare("SELECT run_id,body_json,updated_at FROM run_progress ORDER BY updated_at,run_id").map_err(sql_error)?;
            let rows = statement.query_map([],|row|Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?,row.get::<_,i64>(2)?))).map_err(sql_error)?;
            rows.map(|row| { let (run_id,body,updated_at)=row.map_err(sql_error)?; Ok(ProgressCheckpoint {run_id,body:serde_json::from_str(&body).map_err(json_error)?,updated_at}) }).collect()
        })
    }

    async fn clear_progress(&self, run_id: &str) -> Result<(), ControlStoreError> {
        self.access(|state| {
            let Some(id) = read::location(state, Kind::Run, run_id)? else {
                return Ok(());
            };
            let owned = state
                .projects
                .get(&id)
                .ok_or_else(|| other("Project is not open for progress removal"))?;
            owned
                .connection
                .execute("DELETE FROM run_progress WHERE run_id=?1", [run_id])
                .map_err(sql_error)?;
            Ok(())
        })
    }
}

fn catalog_lock(path: &Path) -> Result<File, ControlStoreError> {
    let file = File::options()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)
        .map_err(io_error)?;
    file.lock().map_err(io_error)?;
    Ok(file)
}

fn random_id(connection: &Connection) -> Result<String, ControlStoreError> {
    connection
        .query_row("SELECT lower(hex(randomblob(16)))", [], |row| row.get(0))
        .map_err(sql_error)
}

fn catalog_id(connection: &Connection) -> Result<String, ControlStoreError> {
    connection
        .query_row(
            "SELECT catalog_id FROM portable_catalog WHERE singleton=1",
            [],
            |row| row.get(0),
        )
        .map_err(sql_error)
}

#[allow(
    clippy::needless_pass_by_value,
    reason = "map_err takes ownership of the I/O error"
)]
fn io_error(error: std::io::Error) -> ControlStoreError {
    other(error.to_string())
}
fn other(message: impl Into<String>) -> ControlStoreError {
    ControlStoreError::Other(message.into())
}

#[cfg(test)]
mod tests;
