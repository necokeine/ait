//! Reliable API terminal persistence, including supervisor failures.
use super::{RunRecord, api_run, is_terminal_run_status};
use crate::control::{
    LocalControlService,
    approvals::expire_pending_native_approvals,
    conversation::{
        messages::{append_output, message},
        release_session,
    },
    errors::{error, recovery_error},
    events::pending,
};
use ait_contracts::ApiError;
use ait_domain::{DomainError, ErrorCode, LifecyclePhase, LifecycleStatus, NativeApprovalStatus};
use ait_ports::ControlStoreError;

async fn wait_for_terminal_persistence(failures: &mut u32) {
    let delay_ms = 1_u64 << (*failures).min(8);
    *failures = failures.saturating_add(1);
    tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
}

impl LocalControlService {
    pub(in crate::control) async fn finish_execution(
        &self,
        run_id: &str,
        result: Result<Option<String>, DomainError>,
    ) -> Result<RunRecord, ApiError> {
        let mut persistence_failures = 0_u32;
        loop {
            let Ok(loaded) = self.read_run_records(run_id).await else {
                wait_for_terminal_persistence(&mut persistence_failures).await;
                continue;
            };
            let mut state = loaded.original.clone();
            let index = state
                .runs
                .iter()
                .position(|run| run.id == run_id)
                .ok_or_else(|| error(ErrorCode::InvalidRun, "run not found", false))?;
            let mut run = state.runs[index].clone();
            if is_terminal_run_status(run.status()) && !api_run::needs_terminal_repair(&run) {
                let _ = self.store.clear_progress(run_id).await;
                return Ok(run);
            }
            if run.execution().is_some() && run.status() == LifecycleStatus::Cancelling {
                run.set_status(LifecycleStatus::Cancelled);
                run.set_error(Some(error(
                    ErrorCode::RunCancelled,
                    "API Run cancellation completed after worker settlement",
                    false,
                )));
            } else {
                match &result {
                    Ok(Some(text)) => {
                        let parent = run
                            .last_message_id()
                            .unwrap_or_else(|| run.base_message_id.clone());
                        let reply = message(
                            &run.project_id,
                            Some(&parent),
                            ait_domain::MessageRole::Assistant,
                            ait_domain::MessageKind::Standard,
                            Some(text.clone()),
                            None,
                            None,
                        );
                        append_output(&mut state, &mut run, reply);
                        run.set_status(LifecycleStatus::Completed);
                        run.set_error(None);
                    }
                    Ok(None) => {}
                    Err(failure) => {
                        run.set_status(if failure.code == ErrorCode::RunCancelled {
                            LifecycleStatus::Cancelled
                        } else {
                            LifecycleStatus::Failed
                        });
                        run.set_error(Some(error(
                            failure.code,
                            &failure.message,
                            failure.retryable,
                        )));
                    }
                }
            }
            let approval_status = if result
                .as_ref()
                .is_err_and(|failure| failure.code == ErrorCode::RunCancelled)
            {
                NativeApprovalStatus::Cancelled
            } else {
                NativeApprovalStatus::Expired
            };
            expire_pending_native_approvals(&mut run, approval_status);
            run.set_phase(Some(LifecyclePhase::Terminal));
            api_run::interrupt(&mut run);
            api_run::append_terminal_results(&mut state, &mut run)
                .map_err(|_| recovery_error("could not settle API child records"))?;
            release_session(&mut state, &run);
            state.runs[index] = run.clone();
            let event = pending("run.updated", Some(run_id.to_owned()), &run);
            match self.persist_records(&loaded, &state, vec![event]).await {
                Ok(()) => {
                    self.notify_approval_waiters(&run.view());
                    let _ = self.store.clear_progress(run_id).await;
                    return Ok(run);
                }
                Err(ControlStoreError::Conflict | ControlStoreError::Other(_)) => {
                    wait_for_terminal_persistence(&mut persistence_failures).await;
                }
            }
        }
    }
}
