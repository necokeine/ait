use std::cmp::Ordering;
use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;

use server_domain::registry::PersistedProjectRecord;
use server_ports::registry::{
    ActiveProjectInput, MutationKind, MutationListener, MutationSubscription, ProjectMutation,
    ProjectRegistry, RegistryError,
};

use super::core::FileRegistry;
use super::listeners::Listeners;

/// Serialized project registry backed by a Paseo-shaped JSON array.
/// Clones share one cache, write queue and observer set; do not open two independent writers.
#[derive(Clone)]
pub struct FileBackedProjectRegistry {
    pub(super) file: Arc<FileRegistry<PersistedProjectRecord>>,
    listeners: Arc<Listeners<ProjectMutation>>,
    pub(super) id_factory: Arc<dyn Fn() -> Result<String, RegistryError> + Send + Sync>,
}

impl fmt::Debug for FileBackedProjectRegistry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FileBackedProjectRegistry")
            .field("file", &self.file)
            .finish_non_exhaustive()
    }
}

impl FileBackedProjectRegistry {
    /// Create a lazy registry for `path`; no file is created until a successful mutation.
    #[must_use]
    pub fn new(path: PathBuf) -> Self {
        Self {
            file: Arc::new(FileRegistry::new(
                path,
                |record: &PersistedProjectRecord| &record.project_id,
            )),
            listeners: Arc::default(),
            id_factory: Arc::new(|| super::generate_id("prj_")),
        }
    }

    fn notify(
        &self,
        kind: MutationKind,
        id: &str,
        record: Option<PersistedProjectRecord>,
    ) -> Result<(), RegistryError> {
        self.listeners.notify(
            &ProjectMutation {
                kind,
                project_id: id.to_owned(),
                project: record,
            },
            false,
        )
    }
}

impl ProjectRegistry for FileBackedProjectRegistry {
    fn initialize(&self) -> Result<(), RegistryError> {
        self.file.initialize()
    }
    fn exists_on_disk(&self) -> bool {
        self.file.exists()
    }
    fn list(&self) -> Result<Vec<PersistedProjectRecord>, RegistryError> {
        self.file.list()
    }
    fn get(&self, id: &str) -> Result<Option<PersistedProjectRecord>, RegistryError> {
        self.file.get(id)
    }

    fn get_or_create_active_by_root(
        &self,
        input: &ActiveProjectInput,
    ) -> Result<PersistedProjectRecord, RegistryError> {
        let (record, changed) = self.file.mutate(|records| {
            let active = records
                .values()
                .filter(|record| {
                    record.archived_at.as_ref().is_none_or(String::is_empty)
                        && super::paths::equivalent(&record.root_path, &input.root_path)
                })
                .min_by(|left, right| oldest(left, right))
                .cloned();
            if let Some(mut active) = active {
                if active.kind == input.kind && active.project_key == input.project_key {
                    return Ok(((active, false), false));
                }
                active.kind = input.kind;
                active.project_key.clone_from(&input.project_key);
                active.updated_at.clone_from(&input.timestamp);
                records.insert(active.project_id.clone(), active.clone());
                return Ok(((active, true), true));
            }
            let id = loop {
                let id = (self.id_factory)()?;
                if !records.contains_key(&id) {
                    break id;
                }
            };
            let record = PersistedProjectRecord {
                project_id: id,
                root_path: input.root_path.clone(),
                kind: input.kind,
                display_name: input.display_name.clone(),
                project_key: input.project_key.clone(),
                custom_name: None,
                custom_icon_revision: None,
                created_at: input.timestamp.clone(),
                updated_at: input.timestamp.clone(),
                archived_at: None,
            };
            records.insert(record.project_id.clone(), record.clone());
            Ok(((record, true), true))
        })?;
        if changed {
            self.notify(
                MutationKind::Upsert,
                &record.project_id,
                Some(record.clone()),
            )?;
        }
        Ok(record)
    }

    fn upsert(&self, record: &PersistedProjectRecord) -> Result<(), RegistryError> {
        self.file.mutate(|records| {
            records.insert(record.project_id.clone(), record.clone());
            Ok(((), true))
        })?;
        self.notify(
            MutationKind::Upsert,
            &record.project_id,
            Some(record.clone()),
        )
    }

    fn update(
        &self,
        id: &str,
        update: &dyn Fn(&PersistedProjectRecord) -> PersistedProjectRecord,
    ) -> Result<Option<PersistedProjectRecord>, RegistryError> {
        let result = self.file.mutate(|records| {
            let Some(existing) = records.get(id) else {
                return Ok((None, false));
            };
            let record = update(existing);
            records.insert(id.to_owned(), record.clone());
            Ok((Some(record), true))
        })?;
        if let Some(record) = &result {
            self.notify(MutationKind::Upsert, id, Some(record.clone()))?;
        }
        Ok(result)
    }

    fn archive(&self, id: &str, timestamp: &str) -> Result<(), RegistryError> {
        let result = self.file.mutate(|records| {
            let Some(record) = records.get_mut(id) else {
                return Ok((None, false));
            };
            if record.archived_at.as_ref().is_some_and(|v| !v.is_empty()) {
                return Ok((None, false));
            }
            timestamp.clone_into(&mut record.updated_at);
            record.archived_at = Some(timestamp.to_owned());
            Ok((Some(record.clone()), true))
        })?;
        if let Some(record) = result {
            self.notify(MutationKind::Archive, id, Some(record))?;
        }
        Ok(())
    }

    fn remove(&self, id: &str) -> Result<(), RegistryError> {
        let removed = self.file.mutate(|records| {
            let removed = records.shift_remove(id).is_some();
            Ok((removed, removed))
        })?;
        if removed {
            self.notify(MutationKind::Remove, id, None)?;
        }
        Ok(())
    }

    fn subscribe_to_mutations(
        &self,
        listener: MutationListener<ProjectMutation>,
    ) -> Box<dyn MutationSubscription> {
        self.listeners.subscribe(listener)
    }
}

fn oldest(left: &PersistedProjectRecord, right: &PersistedProjectRecord) -> Ordering {
    let date_order = match (timestamp(&left.created_at), timestamp(&right.created_at)) {
        (Some(left), Some(right)) => left.cmp(&right),
        _ => Ordering::Equal,
    };
    date_order.then_with(|| left.project_id.cmp(&right.project_id))
}

fn timestamp(value: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|v| v.timestamp_millis())
        .or_else(|| {
            chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d")
                .ok()?
                .and_hms_opt(0, 0, 0)
                .map(|v| v.and_utc().timestamp_millis())
        })
}
