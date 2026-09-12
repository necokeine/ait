//! Workspace Agent invocation and transport-independent supervision.
use crate::control::LocalControlService;
use crate::control::conversation::messages::codex_prompt;
use crate::control::errors::{api_domain_error, error};
use crate::control::permissions::validate_run_permission_ceiling;
use crate::control::project::worktrees::run_workdir;
use crate::control::runs::finalization::{
    InvocationGuard, WorkspaceRunControl, WorkspaceRunControlGuard,
};
use crate::control::runs::journal::WorkspaceExecutionLease;
use crate::control::runs::progress::ProgressPump;
use crate::control::runs::recovery::WorkspaceRecoveryClaim;
use crate::control::state::WorkingSet;
use ait_contracts::{AgentMode, ApiError, RunView};
use ait_domain::{DomainError, ErrorCode};
use ait_ports::{
    WorkspaceAgentInvocation, WorkspaceAgentResponse, WorkspaceApproval, WorkspaceResultSink,
};
use futures_util::FutureExt;
use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

struct DurableWorkspaceResultSink {
    service: LocalControlService,
    lease: WorkspaceExecutionLease,
    checkpointed: AtomicBool,
}

impl DurableWorkspaceResultSink {
    fn is_checkpointed(&self) -> bool {
        self.checkpointed.load(Ordering::Acquire)
    }
}

#[async_trait::async_trait]
impl WorkspaceResultSink for DurableWorkspaceResultSink {
    async fn checkpoint_worker(
        &self,
        operation: &ait_ports::WorkspaceWorkerOperation,
        result: WorkspaceAgentResponse,
    ) -> Result<(), DomainError> {
        self.service
            .persist_workspace_result(&self.lease, result, Some(operation))
            .await
            .map_err(api_domain_error)?;
        self.checkpointed.store(true, Ordering::Release);
        Ok(())
    }
    async fn checkpoint(&self, result: WorkspaceAgentResponse) -> Result<(), DomainError> {
        self.service
            .persist_workspace_result(&self.lease, result, None)
            .await
            .map_err(api_domain_error)?;
        self.checkpointed.store(true, Ordering::Release);
        Ok(())
    }
}

pub(in crate::control) fn workspace_invocation(
    state: &WorkingSet,
    run: &RunView,
    control: Arc<WorkspaceRunControl>,
    approvals: Arc<dyn WorkspaceApproval>,
) -> Result<WorkspaceAgentInvocation, DomainError> {
    let user_text = state
        .messages
        .iter()
        .find(|message| message.id == run.base_message_id)
        .and_then(|message| message.text.clone())
        .ok_or_else(|| DomainError::invariant(ErrorCode::MessageNotFound, "run input not found"))?;
    let message_baseline = state
        .messages
        .iter()
        .find(|message| message.id == run.base_message_id)
        .and_then(|message| message.git_commit.clone());
    let baseline_commit = run
        .workspace_base_commit
        .as_ref()
        .or(message_baseline.as_ref())
        .cloned()
        .ok_or_else(|| {
            DomainError::invariant(
                ErrorCode::ProjectGitHeadUnavailable,
                "Codex Run has no authorized Git baseline",
            )
        })?;
    let baseline_index_tree = run
        .workspace_base_index_tree
        .as_deref()
        .map(str::to_owned)
        .ok_or_else(|| {
            DomainError::invariant(
                ErrorCode::ProjectGitHeadUnavailable,
                "Codex Run has no authorized Git index baseline",
            )
        })?;
    let (project_instructions, prompt) =
        codex_prompt(state, &run.base_message_id).map_err(api_domain_error)?;
    Ok(WorkspaceAgentInvocation {
        request_id: run.id.clone(),
        model: run.config.model.clone(),
        reasoning_effort: run.config.reasoning_effort.clone(),
        project_instructions,
        prompt,
        commit_subject: user_text,
        cwd: run_workdir(state, run).map_err(api_domain_error)?,
        baseline_commit,
        baseline_index_tree,
        permission_profile: run.permission_profile,
        approvals,
        cancellation: control.cancellation.clone(),
        integration_gate: Some(control),
    })
}

impl LocalControlService {
    #[allow(
        clippy::too_many_lines,
        reason = "workspace result checkpoints and recovery are one guarded settlement path"
    )]
    async fn execute_workspace_agent(
        &self,
        run_id: &str,
        control: Arc<WorkspaceRunControl>,
    ) -> Result<RunView, ApiError> {
        let loaded = self.read_run_records(run_id).await?;
        let state = loaded.original;
        let run = state
            .runs
            .iter()
            .find(|run| run.id == run_id)
            .ok_or_else(|| error(ErrorCode::InvalidRun, "run not found", false))?;
        let run = run.clone();
        if matches!(run.provider.kind, AgentMode::OpenAI | AgentMode::DeepSeek) {
            return self
                .execute_api_run(&run, control.cancellation.clone())
                .await;
        }
        let Some(lease) = self.set_run_running(&run.id).await? else {
            let state = self.read_run_records(&run.id).await?.original;
            return state
                .runs
                .into_iter()
                .find(|candidate| candidate.id == run.id)
                .ok_or_else(|| error(ErrorCode::InvalidRun, "run not found", false));
        };
        control.bind_integration_lease(self.clone(), lease.clone())?;
        let progress = ProgressPump::start(self.store.clone(), &run);
        let reporter = progress.reporter();
        let cancellation = control.cancellation.clone();
        let result_sink = DurableWorkspaceResultSink {
            service: self.clone(),
            lease: lease.clone(),
            checkpointed: AtomicBool::new(false),
        };
        let call = async {
            // Queued Runs can outlive the daemon policy that admitted them.
            // Keep their snapshot immutable, but recheck the current ceiling
            // before invoking either provider after startup recovery.
            validate_run_permission_ceiling(run.permission_profile, self.permission_limits)?;
            match run.provider.kind {
                AgentMode::OpenAI | AgentMode::DeepSeek => Err(DomainError::invariant(
                    ErrorCode::InvalidRun,
                    "API Run must use the host coordinator",
                )),
                AgentMode::Codex => {
                    self.invoke_codex_workspace_checkpointed(
                        &state,
                        &run,
                        control.clone(),
                        reporter.clone(),
                        &result_sink,
                    )
                    .await
                }
                #[cfg(all(feature = "dev-mock-provider", debug_assertions))]
                AgentMode::Mock => Ok(Self::invoke_mock()),
            }
        };
        let result = AssertUnwindSafe(async {
            if run.provider.kind == AgentMode::Codex {
                // Workspace execution owns its complete settlement path. The
                // supervisor containing this call survives transport cancellation.
                call.await
            } else {
                tokio::pin!(call);
                tokio::select! {
                    biased;
                    () = cancellation.cancelled() => {
                        Err(DomainError::invariant(ErrorCode::RunCancelled, "run was cancelled"))
                    },
                    result = &mut call => result,
                }
            }
        })
        .catch_unwind()
        .await;
        drop(reporter);
        // Progress storage is deliberately best-effort: a display-channel
        // failure must not prevent Git integration or terminal persistence.
        // This drain also runs after a provider panic, before any terminal
        // commit can clear the checkpoint.
        let _ = progress.finish().await;
        let result = result.unwrap_or_else(|_| {
            Err(DomainError::invariant(
                ErrorCode::ProviderFailed,
                "workspace agent task panicked",
            ))
        });
        if result.is_ok() && run.provider.kind != AgentMode::Codex {
            // Settling is an informational state for the live UI. Once the
            // workspace result exists, failure to expose that intermediate
            // state must not bypass the reliable terminal persistence path.
            let _ = self.set_run_settling(&lease).await;
        }
        if run.provider.kind == AgentMode::Codex && result.is_ok() && !result_sink.is_checkpointed()
        {
            return self
                .finish_workspace_run(
                    &lease,
                    Err(DomainError::invariant(
                        ErrorCode::RunRecoveryFailed,
                        "workspace adapter returned without durably checkpointing its result",
                    )),
                )
                .await;
        }
        // A worker can die after the result checkpoint or Git publication but
        // before its final reply. Reuse NEC-212's durable claim/reconciliation;
        // this increments the epoch and never invokes the model again.
        if run.provider.kind == AgentMode::Codex && result.is_err() && self.run_dispatcher.is_some()
        {
            let latest = self.read_run_records(&run.id).await?.original;
            if latest
                .runs
                .iter()
                .any(|r| r.id == run.id && r.status == "settling")
            {
                match self.claim_startup_recovery(&run.id).await? {
                    WorkspaceRecoveryClaim::Finalize(recovery_lease) => {
                        let recovery_control = Arc::new(WorkspaceRunControl::new());
                        recovery_control
                            .bind_integration_lease(self.clone(), recovery_lease.clone())?;
                        let _invocation = InvocationGuard::new(
                            Arc::clone(&self.cancellations),
                            &run.id,
                            recovery_control.cancellation.clone(),
                        );
                        let _control = WorkspaceRunControlGuard::new(
                            Arc::clone(&self.workspace_run_controls),
                            &run.id,
                            &recovery_control,
                        );
                        return self
                            .recover_checkpointed_run(&recovery_lease, recovery_control)
                            .await;
                    }
                    WorkspaceRecoveryClaim::Recovered(run) => return Ok(*run),
                    WorkspaceRecoveryClaim::Execute | WorkspaceRecoveryClaim::Skip => {}
                }
            }
        }
        self.finish_workspace_run(&lease, result).await
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "the invocation keeps immutable admission, finalization, and progress context together"
    )]
    async fn invoke_codex_workspace_checkpointed(
        &self,
        state: &WorkingSet,
        run: &RunView,
        control: Arc<WorkspaceRunControl>,
        progress: Arc<dyn ait_ports::WorkspaceProgressReporter>,
        result_sink: &dyn WorkspaceResultSink,
    ) -> Result<WorkspaceAgentResponse, DomainError> {
        let executor = self.workspace_agent.as_ref().ok_or_else(|| {
            DomainError::invariant(
                ErrorCode::InvalidConfiguration,
                "Codex workspace executor is not configured",
            )
        })?;
        let invocation = workspace_invocation(state, run, control, Arc::new(self.clone()))?;
        executor
            .invoke_with_progress_and_checkpoint(invocation, progress, result_sink)
            .await
    }

    pub(in crate::control) async fn supervise_workspace_agent(
        &self,
        run_id: String,
        control: Arc<WorkspaceRunControl>,
    ) -> Result<RunView, ApiError> {
        let worker = self.clone();
        let worker_run_id = run_id.clone();
        let task_control = Arc::clone(&control);
        let task = tokio::spawn(async move {
            Box::pin(worker.execute_workspace_agent(&worker_run_id, task_control)).await
        });
        let result = match task.await {
            Ok(Ok(run)) => Ok(run),
            Ok(Err(failure)) => {
                // Admission already made this Run visible to an asynchronous
                // caller. Every later error therefore belongs to the Run and
                // must be persisted before its Session and workspace leases
                // are released.
                self.finish_current_workspace_run(&run_id, Err(api_domain_error(failure)))
                    .await
            }
            Err(failure) => {
                self.finish_current_workspace_run(
                    &run_id,
                    Err(DomainError::invariant(
                        ErrorCode::ProviderFailed,
                        format!("workspace execution task failed: {failure}"),
                    )),
                )
                .await
            }
        };
        drop(control);
        result
    }
}
