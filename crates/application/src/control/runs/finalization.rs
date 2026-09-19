//! Live Run cancellation and transport-independent guard lifetimes.
use crate::control::LocalControlService;
use crate::control::errors::error;
use crate::control::execution::CommandOutcome;
use ait_contracts::{ApiError, Command, CommandResult, NativeApprovalAction};
use ait_domain::ErrorCode;
use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, Weak};

#[derive(Debug)]
pub(in crate::control) struct RunControl {
    pub(in crate::control) cancellation: tokio_util::sync::CancellationToken,
}
impl RunControl {
    pub(in crate::control) fn new() -> Self {
        Self {
            cancellation: tokio_util::sync::CancellationToken::new(),
        }
    }
}

pub(in crate::control) struct RunControlGuard {
    controls: Arc<Mutex<HashMap<String, Weak<RunControl>>>>,
    id: String,
}

impl RunControlGuard {
    pub(in crate::control) fn new(
        controls: Arc<Mutex<HashMap<String, Weak<RunControl>>>>,
        id: &str,
        control: &Arc<RunControl>,
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

impl Drop for RunControlGuard {
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
        workspace_lease: Option<crate::control::admission::WorkspaceWriteLease>,
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
            self.run_controls
                .lock()
                .expect("workspace run controls")
                .get(run_id)
                .and_then(Weak::upgrade)
        });
        let Some(control) = control else {
            let outcome = self
                .commit_command(command, workspace_lease.clone(), derive_source_locked)
                .await?;
            if let CommandOutcome::Ready(result) = &outcome
                && let CommandResult::Run(run) = result.as_ref()
                && crate::control::runs::cancellation_requested(run)
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

        let outcome = self
            .commit_command(command, workspace_lease.clone(), derive_source_locked)
            .await?;
        if let CommandOutcome::Ready(result) = &outcome
            && let CommandResult::Run(run) = result.as_ref()
            && crate::control::runs::cancellation_requested(run)
        {
            control.cancellation.cancel();
        }
        Ok(outcome)
    }
}
