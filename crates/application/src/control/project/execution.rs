//! Project-scoped capability passed to native worker supervision.
use ait_domain::{DomainError, ErrorCode, ProjectOwner};
use ait_ports::{ControlStore, ControlVersion, ProjectExecution};
use async_trait::async_trait;
use std::sync::Arc;

struct Execution {
    store: Arc<dyn ControlStore>,
    owner: ProjectOwner,
}

#[async_trait]
impl ProjectExecution for Execution {
    fn owner(&self) -> ProjectOwner {
        self.owner.clone()
    }
    async fn register_process(&self, pid: u32) -> Result<(), DomainError> {
        self.store
            .register_worker_process(&self.owner, pid)
            .await
            .map_err(|_| {
                DomainError::invariant(
                    ErrorCode::RunQueueConflict,
                    "Project ownership changed before worker admission",
                )
            })
    }
    async fn release_process(&self, pid: u32) -> Result<(), DomainError> {
        self.store
            .release_worker_process(&self.owner, pid)
            .await
            .map_err(|_| {
                DomainError::invariant(
                    ErrorCode::RunQueueConflict,
                    "Project ownership changed during worker drain",
                )
            })
    }
}

impl crate::control::LocalControlService {
    pub(in crate::control) fn project_execution(
        &self,
        version: &ControlVersion,
        project_id: &str,
    ) -> Option<Arc<dyn ProjectExecution>> {
        version.projects.get(project_id).map(|version| {
            Arc::new(Execution {
                store: self.store.clone(),
                owner: version.owner(project_id),
            }) as Arc<dyn ProjectExecution>
        })
    }
}
