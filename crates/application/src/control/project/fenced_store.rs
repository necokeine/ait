//! A continuation retains its original Project acquisition across every CAS retry.
use ait_domain::ProjectOwner;
use ait_ports::{
    ControlChange, ControlFilter, ControlRead, ControlRecordKind, ControlStore, ControlStoreError,
    ControlVersion, DurableEvent, DurableEventPage, EventBounds, PendingEvent, ProgressCheckpoint,
};
use async_trait::async_trait;
use std::sync::Arc;

struct FencedStore {
    inner: Arc<dyn ControlStore>,
    owner: ProjectOwner,
}

impl FencedStore {
    async fn current(&self) -> Result<ControlRead, ControlStoreError> {
        let read = self
            .inner
            .read(&[ControlFilter::id(
                ControlRecordKind::Project,
                &self.owner.project_id,
            )])
            .await?;
        if read
            .version
            .projects
            .get(&self.owner.project_id)
            .map(|version| version.owner(&self.owner.project_id))
            .as_ref()
            != Some(&self.owner)
        {
            return Err(ControlStoreError::Conflict);
        }
        Ok(read)
    }
}

#[async_trait]
impl ControlStore for FencedStore {
    async fn begin_project_drain(&self, id: &str) -> Result<(), ControlStoreError> {
        self.current().await?;
        if id != self.owner.project_id {
            return Err(ControlStoreError::Conflict);
        }
        self.inner.begin_project_drain(id).await
    }
    async fn project_is_open(&self, id: &str) -> Result<bool, ControlStoreError> {
        self.current().await?;
        self.inner.project_is_open(id).await
    }
    async fn close_project(&self, id: &str) -> Result<(), ControlStoreError> {
        self.current().await?;
        if id != self.owner.project_id {
            return Err(ControlStoreError::Conflict);
        }
        self.inner.close_project(id).await
    }
    async fn bind_project_agent(
        &self,
        version: &ControlVersion,
        project_id: &str,
        source_agent_id: &str,
        agent_id: &str,
    ) -> Result<(), ControlStoreError> {
        self.current().await?;
        if project_id != self.owner.project_id {
            return Err(ControlStoreError::Conflict);
        }
        self.inner
            .bind_project_agent(version, project_id, source_agent_id, agent_id)
            .await
    }
    async fn read(&self, filters: &[ControlFilter]) -> Result<ControlRead, ControlStoreError> {
        self.current().await?;
        let mut read = self.inner.read(filters).await?;
        let current = self.current().await?;
        if let Some(version) = read.version.projects.get(&self.owner.project_id) {
            if version.owner(&self.owner.project_id) != self.owner {
                return Err(ControlStoreError::Conflict);
            }
        } else {
            read.version.projects.extend(current.version.projects);
        }
        Ok(read)
    }
    async fn apply(
        &self,
        _revision: u64,
        _changes: Vec<ControlChange>,
        _events: Vec<PendingEvent>,
    ) -> Result<u64, ControlStoreError> {
        Err(ControlStoreError::Conflict)
    }
    async fn apply_versioned(
        &self,
        version: &ControlVersion,
        changes: Vec<ControlChange>,
        events: Vec<PendingEvent>,
    ) -> Result<u64, ControlStoreError> {
        if version
            .projects
            .get(&self.owner.project_id)
            .map(|version| version.owner(&self.owner.project_id))
            .as_ref()
            != Some(&self.owner)
        {
            return Err(ControlStoreError::Conflict);
        }
        self.inner.apply_versioned(version, changes, events).await
    }
    async fn replay(
        &self,
        cursor: u64,
        limit: usize,
    ) -> Result<Vec<DurableEvent>, ControlStoreError> {
        self.current().await?;
        self.inner.replay(cursor, limit).await
    }
    async fn event_bounds(&self) -> Result<EventBounds, ControlStoreError> {
        self.inner.event_bounds().await
    }
    async fn replay_page(
        &self,
        cursor: u64,
        limit: usize,
    ) -> Result<DurableEventPage, ControlStoreError> {
        self.current().await?;
        self.inner.replay_page(cursor, limit).await
    }
    async fn event_namespace(&self) -> Result<String, ControlStoreError> {
        self.inner.event_namespace().await
    }
    async fn save_progress(
        &self,
        checkpoint: ProgressCheckpoint,
        events: Vec<PendingEvent>,
    ) -> Result<(), ControlStoreError> {
        self.current().await?;
        self.inner.save_progress(checkpoint, events).await
    }
    async fn load_progress(&self, id: &str) -> Result<Vec<ProgressCheckpoint>, ControlStoreError> {
        self.current().await?;
        self.inner.load_progress(id).await
    }
    async fn clear_progress(&self, id: &str) -> Result<(), ControlStoreError> {
        self.current().await?;
        self.inner.clear_progress(id).await
    }
    async fn register_worker_process(
        &self,
        owner: &ProjectOwner,
        pid: u32,
    ) -> Result<(), ControlStoreError> {
        if owner != &self.owner {
            return Err(ControlStoreError::Conflict);
        }
        self.inner.register_worker_process(owner, pid).await
    }
    async fn release_worker_process(
        &self,
        owner: &ProjectOwner,
        pid: u32,
    ) -> Result<(), ControlStoreError> {
        if owner != &self.owner {
            return Err(ControlStoreError::Conflict);
        }
        self.inner.release_worker_process(owner, pid).await
    }
}

impl crate::control::LocalControlService {
    /// Scopes a transport request or continuation to one observed Project acquisition.
    /// Stale owners cannot acquire fresh versions through retries.
    #[must_use]
    pub fn with_project_owner(mut self, owner: ProjectOwner) -> Self {
        self.store = Arc::new(FencedStore {
            inner: self.store,
            owner,
        });
        self
    }

    pub(in crate::control) async fn project_service(
        &self,
        project_id: &str,
    ) -> Result<Self, ait_contracts::ApiError> {
        let read = self
            .store
            .read(&[ControlFilter::id(ControlRecordKind::Project, project_id)])
            .await
            .map_err(crate::control::errors::store_error)?;
        Ok(read.version.projects.get(project_id).map_or_else(
            || self.clone(),
            |version| self.clone().with_project_owner(version.owner(project_id)),
        ))
    }

    pub(in crate::control) async fn run_service(
        &self,
        run: &crate::control::runs::RunRecord,
    ) -> Result<Self, ait_contracts::ApiError> {
        let loaded = self.read_run_records(&run.id).await?;
        Ok(loaded.version.projects.get(&run.project_id).map_or_else(
            || self.clone(),
            |version| {
                self.clone()
                    .with_project_owner(version.owner(&run.project_id))
            },
        ))
    }
}
