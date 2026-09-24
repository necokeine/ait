use std::path::PathBuf;
use std::sync::Arc;

use crate::model::registry::PersistedWorkspaceRecord;
use crate::ports::registry::{
    MutationKind, MutationListener, MutationSubscription, RegistryError, WorkspaceArchiveContext,
    WorkspaceMutation, WorkspaceMutationContext, WorkspaceRegistry,
};

use super::core::FileRegistry;
use super::listeners::Listeners;

/// Serialized workspace registry backed by a Paseo-shaped JSON array.
/// Clones share state; reads and writes require the host's data-directory lease.
#[derive(Debug, Clone)]
pub struct FileBackedWorkspaceRegistry {
    pub(super) file: Arc<FileRegistry<PersistedWorkspaceRecord>>,
    listeners: Arc<Listeners<WorkspaceMutation>>,
}

impl FileBackedWorkspaceRegistry {
    /// Create a lazy registry for `path`; missing files stay absent until a mutation.
    #[must_use]
    pub fn new(path: PathBuf) -> Self {
        Self {
            file: Arc::new(FileRegistry::new(
                path,
                |record: &PersistedWorkspaceRecord| &record.workspace_id,
            )),
            listeners: Arc::default(),
        }
    }

    fn notify(&self, mutation: &WorkspaceMutation) -> Result<(), RegistryError> {
        self.listeners.notify(mutation, true)
    }

    pub(crate) fn commit_workspace_label_mutation(
        &self,
        updates: &[PersistedWorkspaceRecord],
        before_write: impl FnOnce(&[PersistedWorkspaceRecord]) -> Result<(), RegistryError>,
        after_write: impl FnOnce() -> Result<(), RegistryError>,
        publish: bool,
    ) -> Result<(), RegistryError> {
        self.file.mutate_with(
            |records| {
                for update in updates {
                    if !records.contains_key(&update.workspace_id) {
                        return Err(RegistryError::InvalidRecord);
                    }
                    records.insert(update.workspace_id.clone(), update.clone());
                }
                Ok(((), true))
            },
            before_write,
            after_write,
        )?;
        if publish {
            for update in updates {
                self.notify(&WorkspaceMutation {
                    kind: MutationKind::Upsert,
                    workspace_id: update.workspace_id.clone(),
                    workspace: Some(update.clone()),
                    expects_initial_agent: None,
                })?;
            }
        }
        Ok(())
    }
}

impl WorkspaceRegistry for FileBackedWorkspaceRegistry {
    fn initialize(&self) -> Result<(), RegistryError> {
        self.file.initialize()
    }
    fn exists_on_disk(&self) -> bool {
        self.file.exists()
    }
    fn list(&self) -> Result<Vec<PersistedWorkspaceRecord>, RegistryError> {
        self.file.list()
    }
    fn get(&self, id: &str) -> Result<Option<PersistedWorkspaceRecord>, RegistryError> {
        self.file.get(id)
    }

    fn upsert(
        &self,
        record: &PersistedWorkspaceRecord,
        context: WorkspaceMutationContext,
    ) -> Result<(), RegistryError> {
        self.file.mutate(|records| {
            records.insert(record.workspace_id.clone(), record.clone());
            Ok(((), true))
        })?;
        self.notify(&WorkspaceMutation {
            kind: MutationKind::Upsert,
            workspace_id: record.workspace_id.clone(),
            workspace: Some(record.clone()),
            expects_initial_agent: context.expects_initial_agent.filter(|v| *v),
        })
    }

    fn update(
        &self,
        id: &str,
        update: &dyn Fn(&PersistedWorkspaceRecord) -> PersistedWorkspaceRecord,
    ) -> Result<Option<PersistedWorkspaceRecord>, RegistryError> {
        let result = self.file.mutate(|records| {
            let Some(existing) = records.get(id) else {
                return Ok((None, false));
            };
            let record = update(existing);
            records.insert(id.to_owned(), record.clone());
            Ok((Some(record), true))
        })?;
        if let Some(record) = &result {
            self.notify(&WorkspaceMutation {
                kind: MutationKind::Upsert,
                workspace_id: id.to_owned(),
                workspace: Some(record.clone()),
                expects_initial_agent: None,
            })?;
        }
        Ok(result)
    }

    fn archive(
        &self,
        id: &str,
        timestamp: &str,
        context: &WorkspaceArchiveContext,
    ) -> Result<(), RegistryError> {
        let result = self.file.mutate(|records| {
            let Some(record) = records.get_mut(id) else {
                return Ok((None, false));
            };
            timestamp.clone_into(&mut record.updated_at);
            record.archived_at = Some(timestamp.to_owned());
            if let Some(url) = &context.auto_archived_change_request_url
                && !url.is_empty()
            {
                record.auto_archived_change_request_url = Some(url.clone());
            }
            Ok((Some(record.clone()), true))
        })?;
        if let Some(record) = result {
            self.notify(&WorkspaceMutation {
                kind: MutationKind::Archive,
                workspace_id: id.to_owned(),
                workspace: Some(record),
                expects_initial_agent: None,
            })?;
        }
        Ok(())
    }

    fn remove(&self, id: &str) -> Result<(), RegistryError> {
        let removed = self.file.mutate(|records| {
            let removed = records.shift_remove(id).is_some();
            Ok((removed, removed))
        })?;
        if removed {
            self.notify(&WorkspaceMutation {
                kind: MutationKind::Remove,
                workspace_id: id.to_owned(),
                workspace: None,
                expects_initial_agent: None,
            })?;
        }
        Ok(())
    }

    fn subscribe_to_mutations(
        &self,
        listener: MutationListener<WorkspaceMutation>,
    ) -> Box<dyn MutationSubscription> {
        self.listeners.subscribe(listener)
    }

    fn block_all_mutations_until_restart(&self) -> Result<(), RegistryError> {
        self.file.freeze()
    }
}
