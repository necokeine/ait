//! Reliable workspace terminal persistence and output publication.
use crate::control::LocalControlService;
use crate::control::approvals::expire_pending_native_approvals;
use crate::control::conversation::messages::{append_output, message};
use crate::control::conversation::release_session;
use crate::control::errors::{error, recovery_error, store_error};
use crate::control::events::pending;
use crate::control::runs::journal::{
    WorkspaceExecutionLease, ensure_current_lease, ensure_journal_lease,
};
use crate::control::runs::{api_run, is_terminal_workspace_status};
use crate::control::state::WorkingSet;
use ait_contracts::{ApiError, RunView};
use ait_domain::{DomainError, ErrorCode, NativeApprovalStatus};
use ait_ports::{ControlStoreError, WorkspaceAgentResponse, WorkspaceOutputItem};
use serde_json::json;

async fn wait_for_workspace_terminal_persistence(failures: &mut u32) {
    let exponent = (*failures).min(8);
    let delay_ms = 1_u64 << exponent;
    *failures = failures.saturating_add(1);
    tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
}

fn apply_workspace_terminal_result(
    state: &mut WorkingSet,
    run: &mut RunView,
    result: &Result<WorkspaceAgentResponse, DomainError>,
) {
    match result {
        Ok(output) => {
            let operations = output
                .operations
                .iter()
                .map(|operation| {
                    json!({
                        "id": operation.id,
                        "kind": operation.kind,
                        "status": operation.status,
                        "title": operation.title,
                        "summary": operation.summary,
                        "detail": operation.detail,
                        "paths": operation.paths,
                    })
                })
                .collect::<Vec<_>>();
            let output_items = output
                .output_items
                .iter()
                .map(|item| match item {
                    WorkspaceOutputItem::Message { id, phase, text } => json!({
                        "type": "message",
                        "id": id,
                        "phase": phase,
                        "text": text,
                    }),
                    WorkspaceOutputItem::Operation { id } => json!({
                        "type": "operation",
                        "id": id,
                    }),
                })
                .collect::<Vec<_>>();
            let data =
                (output.commit_id.is_some() || !operations.is_empty() || !output_items.is_empty())
                    .then(|| {
                        json!({"codex":{
                            "commit_id": output.commit_id,
                            "operations": operations,
                            "output_items": output_items,
                        }})
                    });
            let parent = run
                .last_message_id
                .as_deref()
                .unwrap_or(&run.base_message_id)
                .to_owned();
            let reply = message(
                &run.project_id,
                Some(&parent),
                "assistant",
                "standard",
                Some(output.assistant_text.clone()),
                None,
                data,
            );
            append_output(state, run, reply);
            run.status = "completed".into();
            run.error = None;
        }
        Err(failure) => {
            run.status = match failure.code {
                ErrorCode::RunCancelled => "cancelled",
                ErrorCode::RunLimitExceeded => "limit_exceeded",
                _ => "failed",
            }
            .into();
            run.error = Some(error(failure.code, &failure.message, failure.retryable));
        }
    }
}

impl LocalControlService {
    pub(in crate::control) async fn finish_current_workspace_run(
        &self,
        run_id: &str,
        result: Result<WorkspaceAgentResponse, DomainError>,
    ) -> Result<RunView, ApiError> {
        let lease = self.current_workspace_lease(run_id).await?;
        self.finish_workspace_run(&lease, result).await
    }

    pub(in crate::control) async fn set_run_settling(
        &self,
        lease: &WorkspaceExecutionLease,
    ) -> Result<bool, ApiError> {
        for _ in 0..4 {
            let loaded = self.read_run_records(&lease.run_id).await?;
            let mut state = loaded.original.clone();
            let run = state
                .runs
                .iter_mut()
                .find(|run| run.id == lease.run_id)
                .ok_or_else(|| error(ErrorCode::InvalidRun, "run not found", false))?;
            ensure_current_lease(run, lease)?;
            if run.status != "running" {
                return Ok(false);
            }
            run.status = "settling".into();
            run.phase = Some("settling".into());
            let event = pending("run.updated", Some(lease.run_id.clone()), run);
            match self.persist_records(&loaded, &state, vec![event]).await {
                Ok(()) => return Ok(true),
                Err(ControlStoreError::Conflict) => {}
                Err(error) => return Err(store_error(error)),
            }
        }
        Err(error(
            ErrorCode::RunQueueConflict,
            "concurrent run settling update did not settle",
            true,
        ))
    }

    #[allow(
        clippy::too_many_lines,
        reason = "the terminal snapshot and immutable Message must stay visibly atomic"
    )]
    pub(in crate::control) async fn finish_workspace_run(
        &self,
        lease: &WorkspaceExecutionLease,
        result: Result<WorkspaceAgentResponse, DomainError>,
    ) -> Result<RunView, ApiError> {
        let mut persistence_failures = 0_u32;
        loop {
            let Ok(loaded) = self.read_run_records(&lease.run_id).await else {
                wait_for_workspace_terminal_persistence(&mut persistence_failures).await;
                continue;
            };
            let mut state = loaded.original.clone();
            let index = state
                .runs
                .iter()
                .position(|run| run.id == lease.run_id)
                .ok_or_else(|| error(ErrorCode::InvalidRun, "run not found", false))?;
            let mut run = state.runs[index].clone();
            ensure_current_lease(&run, lease)?;
            if let Some(journal) = state.workspace_run_journals.get(&lease.run_id) {
                ensure_journal_lease(journal, lease)?;
                if let Ok(output) = &result
                    && journal.result.as_ref().is_some_and(|saved| saved != output)
                {
                    return Err(recovery_error(
                        "terminal settlement received a different workspace result",
                    ));
                }
            }
            if is_terminal_workspace_status(&run.status) && !api_run::needs_terminal_repair(&run) {
                let _ = self.store.clear_progress(&lease.run_id).await;
                return Ok(run);
            }
            if run.execution.is_some() && run.status == "cancelling" {
                run.status = "cancelled".into();
                run.error = Some(error(
                    ErrorCode::RunCancelled,
                    "API Run cancellation completed after worker settlement",
                    false,
                ));
            } else if run.status == "settling" && result.is_err() {
                let failure = result.as_ref().expect_err("checked error");
                run.status = "interrupted".into();
                run.error = Some(error(
                    failure.code,
                    format!(
                        "checkpointed workspace result could not be integrated: {}",
                        failure.message
                    ),
                    failure.retryable,
                ));
            } else {
                apply_workspace_terminal_result(&mut state, &mut run, &result);
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
            run.phase = Some("terminal".into());
            api_run::interrupt(&mut run);
            api_run::append_terminal_results(&mut state, &mut run)
                .map_err(|_| recovery_error("could not settle API child records"))?;
            release_session(&mut state, &run);
            state.runs[index] = run.clone();
            let event = pending("run.updated", Some(lease.run_id.clone()), &run);
            match self.persist_records(&loaded, &state, vec![event]).await {
                Ok(()) => {
                    self.notify_approval_waiters(&run);
                    let _ = self.store.clear_progress(&lease.run_id).await;
                    return Ok(run);
                }
                Err(ControlStoreError::Conflict | ControlStoreError::Other(_)) => {
                    // A workspace adapter may already have made its Git result
                    // externally visible. Keep the supervisor's finalization
                    // control and Project/Session leases until the matching
                    // terminal Run, assistant output, and commit audit are all
                    // durable. Returning here would reopen cancellation and
                    // workspace admission around an unaudited integration.
                    wait_for_workspace_terminal_persistence(&mut persistence_failures).await;
                }
            }
        }
    }
}
