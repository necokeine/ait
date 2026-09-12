//! Run cancellation and service shutdown/drain entry points.
use crate::control::LocalControlService;
use crate::control::approvals::expire_pending_native_approvals;
use crate::control::conversation::release_session;
use crate::control::errors::error;
use crate::control::events::pending;
use crate::control::state::WorkingSet;
use ait_contracts::{ApiError, Command, CommandResult};
use ait_domain::{ErrorCode, NativeApprovalStatus};
use ait_ports::PendingEvent;
use std::sync::atomic::Ordering;

pub(in crate::control) mod api_run;
pub(in crate::control) mod finalization;
pub(in crate::control) mod journal;
pub(in crate::control) mod progress;
pub(in crate::control) mod recovery;
pub(in crate::control) mod settlement;
pub(in crate::control) mod workspace;

pub(in crate::control) fn is_terminal_workspace_status(status: &str) -> bool {
    matches!(
        status,
        "completed" | "failed" | "cancelled" | "limit_exceeded" | "interrupted"
    )
}

pub(in crate::control) fn cancel_run(
    state: &mut WorkingSet,
    run_id: &str,
) -> Result<(CommandResult, Vec<PendingEvent>), ApiError> {
    let index = state
        .runs
        .iter()
        .position(|run| run.id == run_id)
        .ok_or_else(|| error(ErrorCode::InvalidRun, "run not found", false))?;
    if is_terminal_workspace_status(&state.runs[index].status) {
        return Err(error(
            ErrorCode::RunAlreadyTerminal,
            "run is already terminal",
            false,
        ));
    }
    if state.runs[index].phase.as_deref() == Some("integrating") {
        return Err(error(
            ErrorCode::RunAlreadyTerminal,
            "workspace integration has started; cancellation cannot replace its durable result",
            false,
        ));
    }
    let mut run = state.runs[index].clone();
    if run.execution.is_some() {
        run.status = "cancelling".into();
        state.runs[index] = run.clone();
        return Ok((
            CommandResult::Run(run.clone()),
            vec![pending("run.updated", Some(run.id.clone()), &run)],
        ));
    }
    run.lease_epoch = run.lease_epoch.saturating_add(1);
    if let Some(journal) = state.workspace_run_journals.get_mut(&run.id) {
        journal.lease_epoch = run.lease_epoch;
    }
    run.status = "cancelled".into();
    run.phase = Some("terminal".into());
    run.error = Some(error(ErrorCode::RunCancelled, "run was cancelled", false));
    expire_pending_native_approvals(&mut run, NativeApprovalStatus::Cancelled);
    release_session(state, &run);
    state.runs[index] = run.clone();
    Ok((
        CommandResult::Run(run.clone()),
        // Terminal Run transitions share one event contract so every client
        // schedules an authoritative view refresh. Renderers still accept
        // legacy run.cancelled events retained in older outboxes.
        vec![pending("run.updated", Some(run.id.clone()), &run)],
    ))
}

impl LocalControlService {
    /// Stop new Run admission, then persist cancellation before signalling workers.
    /// Integrating Git results retain their existing finalization authority.
    /// # Errors
    /// Returns a store failure if shutdown intent cannot be recorded.
    pub async fn begin_shutdown(&self) -> Result<(), ApiError> {
        self.draining.store(true, Ordering::Release);
        // Finish all already-admitted commits before scanning durable Runs.
        let _admission = self.admission.write().await;
        let plan = self.prepare_startup_recovery().await?;
        for run_id in plan.run_ids {
            let response = self.execute(Command::CancelRun { run_id }).await;
            if let Some(failure) = response.error
                && failure.code != ErrorCode::RunAlreadyTerminal
            {
                return Err(failure);
            }
        }
        Ok(())
    }

    /// Whether all application-owned execution/settlement tasks have released their Runs.
    #[must_use]
    pub fn runs_drained(&self) -> bool {
        self.cancellations
            .lock()
            .is_ok_and(|active| active.is_empty())
    }
}
