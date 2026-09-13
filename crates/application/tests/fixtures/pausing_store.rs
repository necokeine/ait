//! Pausing store regression coverage.
#![allow(clippy::pedantic)]
#![allow(dead_code)]
#![allow(missing_docs)]

use crate::support::terminal_run_status;
use ait_ports::ControlStore;
use ait_storage_sqlite::SqliteControlStore;
use async_trait::async_trait;
use tokio::sync::Semaphore;

pub(crate) struct PausingStore {
    pub(crate) inner: SqliteControlStore,
    pub(crate) entered: Semaphore,
    pub(crate) release: Semaphore,
}

#[async_trait]
impl ControlStore for PausingStore {
    async fn read(
        &self,
        filters: &[ait_ports::ControlFilter],
    ) -> Result<ait_ports::ControlRead, ait_ports::ControlStoreError> {
        self.inner.read(filters).await
    }
    async fn replay(
        &self,
        after: u64,
        limit: usize,
    ) -> Result<Vec<ait_ports::DurableEvent>, ait_ports::ControlStoreError> {
        self.inner.replay(after, limit).await
    }
    async fn event_bounds(&self) -> Result<ait_ports::EventBounds, ait_ports::ControlStoreError> {
        self.inner.event_bounds().await
    }
    async fn replay_page(
        &self,
        after: u64,
        limit: usize,
    ) -> Result<ait_ports::DurableEventPage, ait_ports::ControlStoreError> {
        self.inner.replay_page(after, limit).await
    }
    async fn save_progress(
        &self,
        checkpoint: ait_ports::ProgressCheckpoint,
        events: Vec<ait_ports::PendingEvent>,
    ) -> Result<(), ait_ports::ControlStoreError> {
        self.inner.save_progress(checkpoint, events).await
    }
    async fn load_progress(
        &self,
        project_id: &str,
    ) -> Result<Vec<ait_ports::ProgressCheckpoint>, ait_ports::ControlStoreError> {
        self.inner.load_progress(project_id).await
    }
    async fn clear_progress(&self, run_id: &str) -> Result<(), ait_ports::ControlStoreError> {
        self.inner.clear_progress(run_id).await
    }
    async fn apply(
        &self,
        revision: u64,
        changes: Vec<ait_ports::ControlChange>,
        events: Vec<ait_ports::PendingEvent>,
    ) -> Result<u64, ait_ports::ControlStoreError> {
        if terminal_run_status(&changes) == Some("queued") {
            self.entered.add_permits(1);
            self.release.acquire().await.unwrap().forget();
        }
        self.inner.apply(revision, changes, events).await
    }
}
