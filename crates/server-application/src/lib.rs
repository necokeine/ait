//! Project use cases composed exclusively from the independent domain and ports.

pub mod agent_runtime;
pub mod agents;
pub mod checkout;
pub mod daemon;
pub mod directory;
pub mod workspace_automation;
pub mod workspace_labels;
pub mod workspace_state;
pub mod worktrees;

use std::collections::BTreeMap;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use server_domain::{MessageId, OwnerEpoch, Project, ProjectId, RootMessage};
use server_ports::{
    Catalog, CatalogEntry, IdentityLease, Lease, ProjectStorage, ProjectStore, Receipt, Workspace,
};

pub use server_ports::ProjectError;

/// Bounded catalog page size; instruction content is not included in list responses.
pub const MAX_PROJECT_PAGE: usize = 50;

/// Catalog facts plus ownership held by this process (not a claim about other processes).
#[derive(Debug, Clone)]
pub struct ProjectView {
    /// Last registered project facts and canonical root.
    pub entry: CatalogEntry,
    /// Present only while this process holds both project leases.
    pub owner_epoch: Option<OwnerEpoch>,
}

#[derive(Debug)]
struct OpenProject {
    // Rust drops fields in declaration order: storage first, then ID and path leases.
    store: Box<dyn ProjectStore>,
    identity: Box<dyn IdentityLease>,
    _path: Box<dyn Lease>,
    epoch: OwnerEpoch,
    entry: CatalogEntry,
}

/// Blocking, serialized project application service. The host bounds and supervises calls.
#[derive(Debug)]
pub struct Projects {
    open: BTreeMap<ProjectId, OpenProject>,
    catalog: Box<dyn Catalog>,
    storage: Box<dyn ProjectStorage>,
    workspace: Box<dyn Workspace>,
}

impl Projects {
    /// Compose independent adapters; dropping this service releases all idle project leases.
    #[must_use]
    pub fn new(
        catalog: Box<dyn Catalog>,
        storage: Box<dyn ProjectStorage>,
        workspace: Box<dyn Workspace>,
    ) -> Self {
        Self {
            open: BTreeMap::new(),
            catalog,
            storage,
            workspace,
        }
    }

    /// Open an independent repository with a durable, catalog-scoped `key`.
    /// Completed retries return the original receipt without reacquiring a closed project.
    ///
    /// # Errors
    /// Rejects invalid keys, unsupported repositories, conflicting retries, contention, or I/O.
    /// Failed initialization is retained and can be resumed with the same key.
    pub fn open(&mut self, path: &Path, key: &str) -> Result<Receipt, ProjectError> {
        validate_key(key)?;
        let info = self.workspace.inspect(path)?;
        let intent = self.catalog.begin_open(key, &info.root)?;
        if let Some(receipt) = intent.receipt {
            return Ok(receipt);
        }
        if let Some(open) = self
            .open
            .values_mut()
            .find(|open| open.entry.path == info.root)
        {
            open.store.check_owner(open.epoch)?;
            return self.catalog.finish_open(&intent, &open.entry);
        }
        let path_lease = self.workspace.acquire_path(&info)?;
        let mut store = self.storage.open(&info.root)?;
        let name = info
            .root
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or(ProjectError::Invalid)?;
        let created_at = u64::try_from(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(|_| ProjectError::Io)?
                .as_millis(),
        )
        .map_err(|_| ProjectError::Invalid)?;
        let initial = Project::new(
            ProjectId::generate(),
            name.to_owned(),
            info.head,
            RootMessage::new(MessageId::generate(), info.instructions, created_at)?,
        )?;
        let project = store.initialize(&initial)?;
        if self.open.contains_key(&project.id()) {
            return Err(ProjectError::IdentityConflict);
        }
        let identity = self.workspace.acquire_identity(project.id())?;
        let entry = CatalogEntry::from_project(&project, info.root);
        let mut acquired = OpenProject {
            store,
            identity,
            _path: path_lease,
            epoch: OwnerEpoch::new(0)?,
            entry,
        };
        acquired.epoch = acquired
            .identity
            .reserve_epoch(acquired.store.owner_epoch()?)?;
        acquired.store.claim(acquired.epoch)?;
        let receipt = self.catalog.finish_open(&intent, &acquired.entry)?;
        self.open.insert(project.id(), acquired);
        Ok(receipt)
    }

    /// Read one catalog entry and this process's current lease state, without opening it.
    ///
    /// # Errors
    /// Returns `NotFound` or a storage error.
    pub fn get(&mut self, id: ProjectId) -> Result<ProjectView, ProjectError> {
        let entry = self.catalog.get(id)?;
        Ok(self.view(entry))
    }

    /// Page catalog summaries by stable project ID; `limit` must be 1–50.
    ///
    /// # Errors
    /// Rejects invalid limits and propagates catalog failures.
    pub fn list(
        &mut self,
        after: Option<ProjectId>,
        limit: usize,
    ) -> Result<Vec<ProjectView>, ProjectError> {
        if !(1..=MAX_PROJECT_PAGE).contains(&limit) {
            return Err(ProjectError::Invalid);
        }
        Ok(self
            .catalog
            .list(after, limit)?
            .into_iter()
            .map(|entry| self.view(entry))
            .collect())
    }

    /// Persist a close receipt and release this process's idle project resources.
    /// Receipt replay precedes the `expected` owner check and never closes a later reopening.
    ///
    /// # Errors
    /// Rejects conflicting keys, missing ownership, stale generations, or storage failures.
    pub fn close(
        &mut self,
        id: ProjectId,
        expected: OwnerEpoch,
        key: &str,
    ) -> Result<Receipt, ProjectError> {
        validate_key(key)?;
        if let Some(receipt) = self.catalog.close_receipt(key, id)? {
            return Ok(receipt);
        }
        let open = self.open.get_mut(&id).ok_or(ProjectError::NotOpen)?;
        if expected != open.epoch {
            return Err(ProjectError::StaleOwner);
        }
        open.store.check_owner(expected)?;
        let receipt = self.catalog.finish_close(key, id)?;
        self.open.remove(&id);
        Ok(receipt)
    }

    fn view(&self, entry: CatalogEntry) -> ProjectView {
        let owner_epoch = self.open.get(&entry.id).map(|open| open.epoch);
        ProjectView { entry, owner_epoch }
    }
}

fn validate_key(key: &str) -> Result<(), ProjectError> {
    if key.is_empty() || key.len() > 128 || !key.bytes().all(|byte| byte.is_ascii_graphic()) {
        return Err(ProjectError::Invalid);
    }
    Ok(())
}

#[cfg(test)]
mod tests;
