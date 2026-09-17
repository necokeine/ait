//! Startup scanning, recovery claims and checkpoint reconciliation.
use crate::control::LocalControlService;
use crate::control::approvals::expire_pending_native_approvals;
use crate::control::conversation::release_session;
use crate::control::errors::{error, recovery_error, store_error};
use crate::control::events::pending;
use crate::control::model::RunState;
use crate::control::permissions::validate_run_permission_ceiling;
use crate::control::runs::finalization::{InvocationGuard, RunControl, RunControlGuard};
use crate::control::runs::journal::{
    WorkspaceExecutionLease, ensure_current_lease, ensure_journal_lease,
};
use crate::control::runs::workspace::workspace_invocation;
use crate::control::runs::{api_run, is_terminal_run_status};
use crate::control::state::{
    HasMessages, HasRuns, HasSessions, HasSettings, HasWorkspaceRunJournals,
};
use ait_contracts::ApiError;
use ait_domain::{ErrorCode, NativeApprovalStatus};
use ait_domain::{LifecyclePhase, LifecycleStatus};
use ait_ports::ControlStoreError;
use ait_ports::WorkspaceIntegrationGate;
use serde_json::Value;
use std::sync::Arc;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RecoveryPolicy {
    ResumeSafe,
    Ask,
    Fail,
}

#[derive(Clone, Debug, PartialEq)]
pub(in crate::control) enum WorkspaceRecoveryClaim {
    Execute,
    Finalize(WorkspaceExecutionLease),
    Recovered(Box<RunState>),
    Skip,
}

/// Read-only startup scan result. Run ownership changes only after the
/// supervisor acquires the matching Project workspace lease.
pub struct StartupRecoveryPlan {
    pub(in crate::control) run_ids: Vec<String>,
    unavailable_projects: Vec<(String, String)>,
}

impl StartupRecoveryPlan {
    /// Project-specific scan failures; these histories remain untouched.
    #[must_use]
    pub fn unavailable_projects(&self) -> &[(String, String)] {
        &self.unavailable_projects
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.run_ids.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.run_ids.is_empty()
    }
}

fn recovery_policy(state: &impl HasSettings) -> RecoveryPolicy {
    match state
        .settings()
        .0
        .get("runtime.recovery")
        .and_then(Value::as_str)
    {
        Some("ask") => RecoveryPolicy::Ask,
        Some("fail") => RecoveryPolicy::Fail,
        _ => RecoveryPolicy::ResumeSafe,
    }
}

fn is_local_recovery_failure(code: ErrorCode) -> bool {
    matches!(
        code,
        ErrorCode::ProjectWorkspaceBusy
            | ErrorCode::ProjectPathNotFound
            | ErrorCode::ProjectPathNotDirectory
            | ErrorCode::ProjectGitInitFailed
            | ErrorCode::ProjectGitDirty
            | ErrorCode::ProjectGitHeadUnavailable
            | ErrorCode::InvalidProject
            | ErrorCode::InvalidConfiguration
            | ErrorCode::RunNotResumable
    )
}

fn settle_recovered_run(
    state: &mut (impl HasMessages + HasRuns + HasSessions + HasWorkspaceRunJournals),
    index: usize,
    status: LifecycleStatus,
    message: &str,
) -> Result<(), ApiError> {
    let mut run = state.runs()[index].clone();
    run.lease_epoch = run.lease_epoch.saturating_add(1);
    if let Some(journal) = state.workspace_run_journals_mut().get_mut(&run.id) {
        journal.lease_epoch = run.lease_epoch;
    }
    run.set_status(status);
    run.set_phase(Some(LifecyclePhase::Terminal));
    run.set_error(Some(error(
        if status == LifecycleStatus::Cancelled {
            ErrorCode::RunCancelled
        } else {
            ErrorCode::RunRecoveryFailed
        },
        message,
        false,
    )));
    api_run::interrupt(&mut run);
    api_run::append_terminal_results(state, &mut run)
        .map_err(|_| recovery_error("could not settle recovered API children"))?;
    expire_pending_native_approvals(&mut run, NativeApprovalStatus::Expired);
    release_session(state, &run);
    state.runs_mut()[index] = run;
    Ok(())
}

impl LocalControlService {
    /// Scans replay-safe startup work without changing Run ownership. The
    /// returned ids are claimed only after the recovery supervisor owns each
    /// Project workspace lease.
    ///
    /// # Errors
    ///
    /// Returns a persistence error when the global Project catalog cannot be read.
    /// Project-specific failures are reported in the plan without changing their Runs.
    pub async fn prepare_startup_recovery(
        &self,
    ) -> Result<crate::control::StartupRecoveryPlan, ApiError> {
        let catalog = self.records().read_project_catalog().await?.original;
        let mut plan = StartupRecoveryPlan {
            run_ids: Vec::new(),
            unavailable_projects: Vec::new(),
        };
        for project in catalog.projects {
            match self.records().read_project_run_records(&project.id).await {
                Ok(state) => plan.run_ids.extend(
                    state
                        .original
                        .runs
                        .into_iter()
                        .filter(|run| {
                            !is_terminal_run_status(run.status())
                                || api_run::needs_terminal_repair(run)
                                || run.compatibility_repair
                        })
                        .map(|run| run.id),
                ),
                Err(failure) => plan
                    .unavailable_projects
                    .push((project.id, failure.message)),
            }
        }
        Ok(plan)
    }

    /// Executes a previously claimed startup plan after the daemon listener is
    /// ready. Local Project/Git failures interrupt only their owning Run.
    ///
    /// # Errors
    ///
    /// Returns when global durable state can no longer be read or committed.
    pub async fn run_startup_recovery(
        &self,
        plan: StartupRecoveryPlan,
    ) -> Result<Vec<ait_contracts::RunView>, ApiError> {
        self.run_startup_recovery_states(plan)
            .await
            .map(|runs| runs.iter().map(RunState::view).collect())
    }
    async fn run_startup_recovery_states(
        &self,
        plan: StartupRecoveryPlan,
    ) -> Result<Vec<RunState>, ApiError> {
        let mut recovered = Vec::new();
        for run_id in plan.run_ids {
            let workspace_lease = match self.acquire_workspace_write_for_run(&run_id).await {
                Ok(lease) => lease,
                Err(failure) if failure.code == ErrorCode::ProjectWorkspaceBusy => {
                    // A live executor still owns this Project. It also owns the
                    // only path to Git publication, so do not steal its epoch.
                    continue;
                }
                Err(failure) if is_local_recovery_failure(failure.code) => {
                    recovered.push(self.interrupt_recovery_run(&run_id, &failure).await?);
                    continue;
                }
                Err(failure) => return Err(failure),
            };
            let claim = self.claim_startup_recovery(&run_id).await?;
            let lease = match claim {
                WorkspaceRecoveryClaim::Recovered(run) => {
                    recovered.push(*run);
                    continue;
                }
                WorkspaceRecoveryClaim::Skip => continue,
                WorkspaceRecoveryClaim::Execute => None,
                WorkspaceRecoveryClaim::Finalize(lease) => Some(lease),
            };
            let control = Arc::new(RunControl::new());
            if let Some(lease) = lease.as_ref() {
                control.bind_integration_lease(self.clone(), lease.clone())?;
            }
            let invocation = InvocationGuard::new(
                Arc::clone(&self.cancellations),
                &run_id,
                control.cancellation.clone(),
            );
            let control_guard =
                RunControlGuard::new(Arc::clone(&self.run_controls), &run_id, &control);
            let result = match lease {
                None => self.supervise_run(run_id.clone(), control.clone()).await,
                Some(lease) => self.recover_checkpointed_run(&lease, control.clone()).await,
            };
            drop((workspace_lease, invocation, control_guard, control));
            match result {
                Ok(run) => recovered.push(run),
                Err(failure) => return Err(failure),
            }
        }
        Ok(recovered)
    }

    #[allow(
        clippy::too_many_lines,
        reason = "the ownership claim keeps policy, lease fencing, and its event in one CAS"
    )]
    pub(in crate::control) async fn claim_startup_recovery(
        &self,
        run_id: &str,
    ) -> Result<WorkspaceRecoveryClaim, ApiError> {
        for _ in 0..4 {
            let loaded = self.read_run_records(run_id).await?;
            let mut state = loaded.original.clone();
            let index = state
                .runs
                .iter()
                .position(|run| run.id == run_id)
                .ok_or_else(|| error(ErrorCode::InvalidRun, "run not found", false))?;
            if is_terminal_run_status(state.runs[index].status()) {
                if api_run::needs_terminal_repair(&state.runs[index]) {
                    let terminal = if state.runs[index].status() == LifecycleStatus::Cancelled {
                        LifecycleStatus::Cancelled
                    } else {
                        LifecycleStatus::Failed
                    };
                    settle_recovered_run(
                        &mut state,
                        index,
                        terminal,
                        "repaired inconsistent API terminal state; unknown effects will not replay",
                    )?;
                } else {
                    if !state.runs[index].compatibility_repair {
                        return Ok(WorkspaceRecoveryClaim::Skip);
                    }
                    let run = state.runs[index].clone();
                    release_session(&mut state, &run);
                }
                let run = state.runs[index].clone();
                match self
                    .persist_records(
                        &loaded,
                        &state,
                        vec![pending("run.updated", Some(run_id.into()), &run)],
                    )
                    .await
                {
                    Ok(()) => return Ok(WorkspaceRecoveryClaim::Recovered(Box::new(run))),
                    Err(ControlStoreError::Conflict) => continue,
                    Err(e) => return Err(store_error(e)),
                }
            }
            let policy = recovery_policy(&state);
            let status = state.runs[index].status();
            let claim = match policy {
                _ if status == LifecycleStatus::Cancelling => {
                    settle_recovered_run(
                        &mut state,
                        index,
                        LifecycleStatus::Cancelled,
                        "run cancellation was completed during daemon recovery",
                    )?;
                    WorkspaceRecoveryClaim::Recovered(Box::new(state.runs[index].clone()))
                }
                RecoveryPolicy::ResumeSafe if state.runs[index].execution().is_some() => {
                    return Ok(WorkspaceRecoveryClaim::Execute);
                }
                RecoveryPolicy::ResumeSafe if status == LifecycleStatus::Queued => {
                    return Ok(WorkspaceRecoveryClaim::Execute);
                }
                RecoveryPolicy::ResumeSafe if status == LifecycleStatus::Settling => {
                    let operation_id = state.runs[index].operation_id.as_deref().map(str::to_owned);
                    let valid = operation_id.as_ref().is_some_and(|operation_id| {
                        state
                            .workspace_run_journals
                            .get(run_id)
                            .is_some_and(|journal| {
                                journal.operation_id == *operation_id
                                    && journal.lease_epoch == state.runs[index].lease_epoch
                                    && journal.result.is_some()
                            })
                    });
                    if valid {
                        let operation_id = operation_id
                            .ok_or_else(|| recovery_error("validated operation id disappeared"))?;
                        let lease_epoch = state.runs[index].lease_epoch.saturating_add(1);
                        state
                            .workspace_run_journals
                            .get_mut(run_id)
                            .ok_or_else(|| recovery_error("validated result journal disappeared"))?
                            .lease_epoch = lease_epoch;
                        state
                            .workspace_run_journals
                            .get_mut(run_id)
                            .ok_or_else(|| recovery_error("result journal disappeared"))?
                            .worker_instance_id = None;
                        let run = &mut state.runs[index];
                        run.lease_epoch = lease_epoch;
                        run.set_phase(Some(LifecyclePhase::ReconcilingResult));
                        run.set_error(None);
                        WorkspaceRecoveryClaim::Finalize(WorkspaceExecutionLease {
                            run_id: run_id.to_owned(),
                            operation_id,
                            lease_epoch,
                        })
                    } else {
                        settle_recovered_run(
                            &mut state,
                            index,
                            LifecycleStatus::Interrupted,
                            concat!(
                                "checkpointed workspace result is incomplete; recovery material ",
                                "was preserved for review"
                            ),
                        )?;
                        WorkspaceRecoveryClaim::Recovered(Box::new(state.runs[index].clone()))
                    }
                }
                RecoveryPolicy::ResumeSafe => {
                    settle_recovered_run(
                        &mut state,
                        index,
                        LifecycleStatus::Interrupted,
                        concat!(
                            "run effects are not proven replay-safe; isolated workspace recovery ",
                            "material was preserved for review"
                        ),
                    )?;
                    WorkspaceRecoveryClaim::Recovered(Box::new(state.runs[index].clone()))
                }
                RecoveryPolicy::Ask => {
                    settle_recovered_run(
                        &mut state,
                        index,
                        LifecycleStatus::Interrupted,
                        "recovery policy requires user review; workspace changes were preserved",
                    )?;
                    WorkspaceRecoveryClaim::Recovered(Box::new(state.runs[index].clone()))
                }
                RecoveryPolicy::Fail => {
                    settle_recovered_run(
                        &mut state,
                        index,
                        LifecycleStatus::Failed,
                        "run failed because the daemon restarted",
                    )?;
                    WorkspaceRecoveryClaim::Recovered(Box::new(state.runs[index].clone()))
                }
            };
            let run = &state.runs[index];
            let event = pending(
                if is_terminal_run_status(run.status()) {
                    "run.recovered"
                } else {
                    "run.recovery_claimed"
                },
                Some(run.id.clone()),
                run,
            );
            match self.persist_records(&loaded, &state, vec![event]).await {
                Ok(()) => return Ok(claim),
                Err(ControlStoreError::Conflict) => {}
                Err(error) => return Err(store_error(error)),
            }
        }
        Err(error(
            ErrorCode::RunQueueConflict,
            "startup Run ownership claim did not settle",
            true,
        ))
    }

    /// Synchronous convenience for tests and non-daemon embeddings.
    ///
    /// # Errors
    ///
    /// Returns a global persistence error from either startup phase.
    pub async fn recover_interrupted_runs(&self) -> Result<Vec<ait_contracts::RunView>, ApiError> {
        let plan = self.prepare_startup_recovery().await?;
        self.run_startup_recovery(plan).await
    }

    pub(in crate::control) async fn recover_checkpointed_run(
        &self,
        lease: &WorkspaceExecutionLease,
        control: Arc<RunControl>,
    ) -> Result<RunState, ApiError> {
        let state = self.read_run_records(&lease.run_id).await?.original;
        let run = state
            .runs
            .iter()
            .find(|run| run.id == lease.run_id)
            .ok_or_else(|| error(ErrorCode::InvalidRun, "run not found", false))?;
        ensure_current_lease(run, lease)?;
        if let Err(failure) =
            validate_run_permission_ceiling(run.permission_profile, self.permission_limits)
        {
            let failure = error(failure.code, failure.message, false);
            return self.interrupt_recovery_run(&lease.run_id, &failure).await;
        }
        let journal = state
            .workspace_run_journals
            .get(&lease.run_id)
            .ok_or_else(|| recovery_error("workspace result journal is missing"))?;
        ensure_journal_lease(journal, lease)?;
        let result = journal
            .result
            .clone()
            .ok_or_else(|| recovery_error("workspace result checkpoint is missing"))?;
        let executor = self.workspace_agent.as_ref().ok_or_else(|| {
            error(
                ErrorCode::InvalidConfiguration,
                "Codex workspace executor is not configured",
                false,
            )
        })?;
        control
            .begin_integration()
            .await
            .map_err(|failure| error(failure.code, failure.message, failure.retryable))?;
        let invocation = workspace_invocation(&state, run, control, Arc::new(self.clone()))
            .map_err(|failure| error(failure.code, failure.message, failure.retryable))?;
        let output = match executor
            .recover_checkpointed(invocation, result, journal.baseline_ref.clone())
            .await
        {
            Ok(output) => output,
            Err(failure) => {
                let failure = error(failure.code, failure.message, failure.retryable);
                return self.interrupt_recovery_run(&lease.run_id, &failure).await;
            }
        };
        self.finish_workspace_run(lease, Ok(output)).await
    }

    async fn interrupt_recovery_run(
        &self,
        run_id: &str,
        failure: &ApiError,
    ) -> Result<RunState, ApiError> {
        for _ in 0..4 {
            let loaded = self.read_run_records(run_id).await?;
            let mut state = loaded.original.clone();
            let index = state
                .runs
                .iter()
                .position(|run| run.id == run_id)
                .ok_or_else(|| error(ErrorCode::InvalidRun, "run not found", false))?;
            if is_terminal_run_status(state.runs[index].status())
                && !api_run::needs_terminal_repair(&state.runs[index])
            {
                return Ok(state.runs[index].clone());
            }
            let mut run = state.runs[index].clone();
            run.lease_epoch = run.lease_epoch.saturating_add(1);
            run.set_status(
                if run.status() == LifecycleStatus::Cancelling
                    || run.status() == LifecycleStatus::Cancelled
                {
                    LifecycleStatus::Cancelled
                } else {
                    LifecycleStatus::Interrupted
                },
            );
            run.set_phase(Some(LifecyclePhase::Terminal));
            run.set_error(Some(error(
                ErrorCode::RunRecoveryFailed,
                &failure.message,
                false,
            )));
            api_run::interrupt(&mut run);
            api_run::append_terminal_results(&mut state, &mut run)
                .map_err(|_| recovery_error("could not settle interrupted API children"))?;
            expire_pending_native_approvals(&mut run, NativeApprovalStatus::Expired);
            if let Some(journal) = state.workspace_run_journals.get_mut(run_id) {
                journal.lease_epoch = run.lease_epoch;
            }
            release_session(&mut state, &run);
            state.runs[index] = run.clone();
            let event = pending("run.recovery_required", Some(run_id.to_owned()), &run);
            match self.persist_records(&loaded, &state, vec![event]).await {
                Ok(()) => {
                    self.notify_approval_waiters(&run.view());
                    let _ = self.store.clear_progress(run_id).await;
                    return Ok(run);
                }
                Err(ControlStoreError::Conflict) => {}
                Err(error) => return Err(store_error(error)),
            }
        }
        Err(error(
            ErrorCode::RunQueueConflict,
            "interrupted Run recovery did not settle",
            true,
        ))
    }
}
