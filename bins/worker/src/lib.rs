//! Composition boundary for executing one supervised AIT Run.

use std::sync::Arc;

use ait_domain::RunId;
use ait_ports::{RunAgent, RunApproval, RunStore, RunTool};
use ait_runtime::{DriveOutcome, RunCoordinator, RunCoordinatorError, SystemClock, UuidIds};
use tokio_util::sync::CancellationToken;

/// A single-Run worker assembled from daemon-backed state and execution ports.
///
/// The worker owns orchestration only. Its [`RunStore`] implementation remains
/// responsible for making every state transition durable before returning.
pub struct RunWorker {
    coordinator: RunCoordinator,
}

impl RunWorker {
    /// Builds a worker with production clock and identity generators.
    #[must_use]
    pub fn new(
        store: Arc<dyn RunStore>,
        agent: Arc<dyn RunAgent>,
        tools: Arc<dyn RunTool>,
        approvals: Arc<dyn RunApproval>,
    ) -> Self {
        Self {
            coordinator: RunCoordinator::new(
                store,
                agent,
                tools,
                approvals,
                Arc::new(SystemClock),
                Arc::new(UuidIds),
            ),
        }
    }

    /// Drives the assigned Run until it completes or reaches a stable boundary.
    ///
    /// # Errors
    ///
    /// Returns [`RunCoordinatorError`] if authoritative state cannot be read or
    /// committed, or if the persisted Run cannot be resumed safely.
    pub async fn execute(
        &self,
        run_id: &RunId,
        cancellation: CancellationToken,
    ) -> Result<DriveOutcome, RunCoordinatorError> {
        self.coordinator.drive(run_id, cancellation).await
    }
}
