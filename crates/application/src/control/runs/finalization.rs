//! Cancellation versus integration authority and live Run guard lifetimes.
use crate::control::LocalControlService;
use crate::control::errors::{api_domain_error, error, recovery_error};
use crate::control::execution::CommandOutcome;
use crate::control::runs::journal::WorkspaceExecutionLease;
use ait_contracts::{ApiError, Command, CommandResult, NativeApprovalAction};
use ait_domain::{DomainError, ErrorCode};
use ait_ports::WorkspaceIntegrationGate;
use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, OnceLock, Weak};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum WorkspaceFinalizationDecision {
    Open,
    Integrating,
    Cancelled,
}

pub(in crate::control) struct WorkspaceRunControl {
    pub(in crate::control) cancellation: tokio_util::sync::CancellationToken,
    finalization: tokio::sync::Mutex<WorkspaceFinalizationDecision>,
    integration_lease: OnceLock<WorkspaceIntegrationLease>,
}

#[derive(Clone)]
struct WorkspaceIntegrationLease {
    service: LocalControlService,
    lease: WorkspaceExecutionLease,
}

impl std::fmt::Debug for WorkspaceRunControl {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("WorkspaceRunControl")
            .field("cancellation", &self.cancellation)
            .field("finalization", &self.finalization)
            .field(
                "integration_lease_bound",
                &self.integration_lease.get().is_some(),
            )
            .finish()
    }
}

impl WorkspaceRunControl {
    pub(in crate::control) fn new() -> Self {
        Self {
            cancellation: tokio_util::sync::CancellationToken::new(),
            finalization: tokio::sync::Mutex::new(WorkspaceFinalizationDecision::Open),
            integration_lease: OnceLock::new(),
        }
    }

    pub(in crate::control) fn bind_integration_lease(
        &self,
        service: LocalControlService,
        lease: WorkspaceExecutionLease,
    ) -> Result<(), ApiError> {
        self.integration_lease
            .set(WorkspaceIntegrationLease { service, lease })
            .map_err(|_| recovery_error("workspace integration lease was bound more than once"))
    }
}

#[async_trait::async_trait]
impl WorkspaceIntegrationGate for WorkspaceRunControl {
    async fn claim_worker(&self, instance: &str) -> Result<ait_ports::WorkerLease, DomainError> {
        let binding = self.integration_lease.get().ok_or_else(|| {
            api_domain_error(recovery_error("workspace execution lease unavailable"))
        })?;
        binding
            .service
            .claim_workspace_worker(&binding.lease, instance)
            .await
            .map_err(api_domain_error)
    }
    async fn begin_worker_integration(
        &self,
        operation: &ait_ports::WorkspaceWorkerOperation,
    ) -> Result<(), DomainError> {
        self.begin_integration_with_worker(Some(operation)).await
    }
    async fn begin_integration(&self) -> Result<(), DomainError> {
        self.begin_integration_with_worker(None).await
    }
}

impl WorkspaceRunControl {
    async fn begin_integration_with_worker(
        &self,
        operation: Option<&ait_ports::WorkspaceWorkerOperation>,
    ) -> Result<(), DomainError> {
        let mut decision = self.finalization.lock().await;
        match *decision {
            WorkspaceFinalizationDecision::Open if !self.cancellation.is_cancelled() => {}
            WorkspaceFinalizationDecision::Integrating => {}
            WorkspaceFinalizationDecision::Open | WorkspaceFinalizationDecision::Cancelled => {
                return Err(DomainError::invariant(
                    ErrorCode::RunCancelled,
                    "run cancellation won before workspace integration",
                ));
            }
        }
        let binding = self.integration_lease.get().ok_or_else(|| {
            DomainError::invariant(
                ErrorCode::RunRecoveryFailed,
                "workspace integration has no durable execution lease",
            )
        })?;
        binding
            .service
            .claim_workspace_integration(&binding.lease, operation)
            .await
            .map_err(api_domain_error)?;
        *decision = WorkspaceFinalizationDecision::Integrating;
        Ok(())
    }
}

pub(in crate::control) struct WorkspaceRunControlGuard {
    controls: Arc<Mutex<HashMap<String, Weak<WorkspaceRunControl>>>>,
    id: String,
}

impl WorkspaceRunControlGuard {
    pub(in crate::control) fn new(
        controls: Arc<Mutex<HashMap<String, Weak<WorkspaceRunControl>>>>,
        id: &str,
        control: &Arc<WorkspaceRunControl>,
    ) -> Self {
        controls
            .lock()
            .expect("workspace run controls")
            .insert(id.to_owned(), Arc::downgrade(control));
        Self {
            controls,
            id: id.to_owned(),
        }
    }
}

impl Drop for WorkspaceRunControlGuard {
    fn drop(&mut self) {
        self.controls
            .lock()
            .expect("workspace run controls")
            .remove(&self.id);
    }
}

pub(in crate::control) struct InvocationGuard {
    cancellations: Arc<Mutex<HashMap<String, tokio_util::sync::CancellationToken>>>,
    id: String,
}

impl InvocationGuard {
    pub(in crate::control) fn new(
        cancellations: Arc<Mutex<HashMap<String, tokio_util::sync::CancellationToken>>>,
        id: &str,
        token: tokio_util::sync::CancellationToken,
    ) -> Self {
        cancellations
            .lock()
            .expect("cancellations")
            .insert(id.into(), token);
        Self {
            cancellations,
            id: id.into(),
        }
    }
}

impl Drop for InvocationGuard {
    fn drop(&mut self) {
        self.cancellations
            .lock()
            .expect("cancellations")
            .remove(&self.id);
    }
}

impl LocalControlService {
    pub(in crate::control) async fn commit_with_finalization_gate(
        &self,
        command: Command,
        has_workspace_lease: bool,
        derive_source_locked: bool,
    ) -> Result<CommandOutcome, ApiError> {
        let interactive = matches!(
            &command,
            Command::SendMessage { .. }
                | Command::ForkSession { .. }
                | Command::DeriveSession { .. }
        );
        let _admission = if interactive {
            Some(self.admission.read().await)
        } else {
            None
        };
        if interactive && self.draining.load(Ordering::Acquire) {
            return Err(error(ErrorCode::RunCancelled, "daemon is draining", false));
        }
        let cancellation_run_id = match &command {
            Command::CancelRun { run_id }
            | Command::ResolveNativeApproval {
                run_id,
                action: NativeApprovalAction::Cancel,
                ..
            } => Some(run_id),
            _ => None,
        };
        let control = cancellation_run_id.and_then(|run_id| {
            self.workspace_run_controls
                .lock()
                .expect("workspace run controls")
                .get(run_id)
                .and_then(Weak::upgrade)
        });
        let Some(control) = control else {
            let outcome = self
                .commit_command(command, has_workspace_lease, derive_source_locked)
                .await?;
            if let CommandOutcome::Ready(result) = &outcome
                && let CommandResult::Run(run) = result.as_ref()
                && matches!(run.status.as_str(), "cancelled" | "cancelling")
                && let Some(token) = self
                    .cancellations
                    .lock()
                    .expect("cancellations")
                    .get(&run.id)
            {
                token.cancel();
            }
            return Ok(outcome);
        };

        let mut decision = control.finalization.lock().await;
        if *decision == WorkspaceFinalizationDecision::Integrating {
            return Err(error(
                ErrorCode::RunAlreadyTerminal,
                "workspace integration has started; cancellation cannot replace its durable result",
                false,
            ));
        }
        let outcome = self
            .commit_command(command, has_workspace_lease, derive_source_locked)
            .await?;
        if let CommandOutcome::Ready(result) = &outcome
            && let CommandResult::Run(run) = result.as_ref()
            && matches!(run.status.as_str(), "cancelled" | "cancelling")
        {
            *decision = WorkspaceFinalizationDecision::Cancelled;
            control.cancellation.cancel();
        }
        Ok(outcome)
    }
}
