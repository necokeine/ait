use std::{collections::HashSet, future::Future, sync::Arc, time::SystemTime};

use ait_domain::{
    CostMicros, DomainError, DomainMetadata, ErrorCode, Message, MessageId, MessageKind,
    MessageOrigin, MessageRole, ProjectedMessage, Run, RunAttempt, RunAttemptReason,
    RunAttemptStatus, RunId, RunPhase, RunStatus, RunStopReason, RunUsage, SubMessage, TimestampMs,
    ToolApprovalStatus, ToolExecution, ToolExecutionStatus, ToolResult, ToolResultStatus, ToolUse,
};
use ait_ports::{
    AgentInvocation, ApprovalDecision, ApprovalRequest, CompletionResult, RunAgent, RunApproval,
    RunClock, RunIdGenerator, RunStore, RunStoreError, RunTool, ToolInvocation, ToolOutcome,
    ToolRecovery,
};
use thiserror::Error;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

/// A stable boundary reached by one foreground drive operation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DriveOutcome {
    /// The termination barrier committed a successful result.
    Completed(Run),
    /// A persisted tool intent is waiting for an external approval decision.
    WaitingApproval(Run),
    /// The Run ended unsuccessfully, by cancellation, or by a configured limit.
    Terminal(Run),
}

/// Coordinator failure before a truthful Run transition could be persisted.
#[derive(Debug, Error)]
pub enum RunCoordinatorError {
    /// Durable state could not be read or committed.
    #[error(transparent)]
    Store(#[from] RunStoreError),
    /// Persisted state cannot be resumed safely.
    #[error("{0}")]
    InvalidState(String),
}

/// Foreground Run supervisor implementing the persisted Agent/tool loop.
pub struct RunCoordinator {
    store: Arc<dyn RunStore>,
    agent: Arc<dyn RunAgent>,
    tools: Arc<dyn RunTool>,
    approvals: Arc<dyn RunApproval>,
    clock: Arc<dyn RunClock>,
    ids: Arc<dyn RunIdGenerator>,
}

impl RunCoordinator {
    /// Creates a coordinator from replaceable persistence and execution ports.
    #[must_use]
    pub fn new(
        store: Arc<dyn RunStore>,
        agent: Arc<dyn RunAgent>,
        tools: Arc<dyn RunTool>,
        approvals: Arc<dyn RunApproval>,
        clock: Arc<dyn RunClock>,
        ids: Arc<dyn RunIdGenerator>,
    ) -> Self {
        Self {
            store,
            agent,
            tools,
            approvals,
            clock,
            ids,
        }
    }

    /// Drives a Run until it completes, stops, or reaches an approval boundary.
    ///
    /// Every Agent attempt, assistant Message, tool intent/outcome, `ToolResult`,
    /// wait and terminal transition is durable before dependent work begins.
    /// Calling this method again is the recovery mechanism for non-terminal
    /// Runs left at a safe checkpoint by a crash.
    ///
    /// # Errors
    ///
    /// Returns [`RunCoordinatorError`] only when durable state cannot be read or
    /// committed, or persisted state is not safely resumable.
    pub async fn drive(
        &self,
        run_id: &RunId,
        cancellation: CancellationToken,
    ) -> Result<DriveOutcome, RunCoordinatorError> {
        let mut run = self.store.load_run(run_id).await?;
        run.validate()
            .map_err(|error| RunCoordinatorError::InvalidState(error.to_string()))?;
        if run.status.is_terminal() {
            return Ok(Self::terminal_outcome(run));
        }

        if run.status == RunStatus::Queued {
            run.status = RunStatus::Running;
            run.phase = RunPhase::AssemblingContext;
            run.started_at = Some(self.clock.now());
            run = self.store.save_run(run).await?;
        }

        loop {
            if let Some(reason) = self.limit_or_cancel(&run, &cancellation) {
                let status = if reason == RunStopReason::Cancelled {
                    RunStatus::Cancelled
                } else {
                    RunStatus::LimitExceeded
                };
                run = self.finish(run, status, reason, None).await?;
                return Ok(DriveOutcome::Terminal(run));
            }

            if run.status == RunStatus::RetryWait {
                let deadline = run.next_retry_at.ok_or_else(|| {
                    RunCoordinatorError::InvalidState("retry wait has no deadline".into())
                })?;
                if self.clock.now() < deadline {
                    tokio::select! {
                        () = self.clock.sleep_until(deadline) => {}
                        () = cancellation.cancelled() => continue,
                    }
                }
                run.status = RunStatus::Running;
                run.phase = RunPhase::AssemblingContext;
                run.next_retry_at = None;
                run = self.store.save_run(run).await?;
                continue;
            }

            let head = run.last_message_id.unwrap_or(run.base_message_id);
            let path = self.store.load_message_path(&head).await?;
            if let Some(assistant) = pending_tool_assistant(&path).cloned() {
                match self
                    .process_tools(run, assistant, cancellation.clone())
                    .await?
                {
                    ToolLoop::Continue(next) => run = next,
                    ToolLoop::Waiting(next) => {
                        return Ok(DriveOutcome::WaitingApproval(next));
                    }
                    ToolLoop::Terminal(next) => return Ok(DriveOutcome::Terminal(next)),
                }
                continue;
            }
            match visible_tail(&path) {
                Some(message) if message.role == MessageRole::Assistant => {
                    run = self.settle(run).await?;
                    let expected_queue_version = run.queue_version;
                    run.status = RunStatus::Completed;
                    run.phase = RunPhase::Terminal;
                    run.stop_reason = Some(RunStopReason::Completed);
                    run.ended_at = Some(self.clock.now());
                    match self.store.try_complete(run, expected_queue_version).await? {
                        CompletionResult::Completed(completed) => {
                            return Ok(DriveOutcome::Completed(completed));
                        }
                        CompletionResult::QueueChanged(changed) => {
                            run = changed;
                            run.status = RunStatus::Running;
                            run.phase = RunPhase::DrainingQueue;
                            run.stop_reason = None;
                            run.ended_at = None;
                            run = self.store.save_run(run).await?;
                            run = self.store.drain_queue(run).await?;
                        }
                    }
                }
                _ => {
                    run = self.invoke_agent(run, path, cancellation.clone()).await?;
                    if run.status.is_terminal() {
                        return Ok(DriveOutcome::Terminal(run));
                    }
                }
            }
        }
    }

    #[allow(clippy::too_many_lines)]
    async fn invoke_agent(
        &self,
        mut run: Run,
        path: Vec<ProjectedMessage>,
        cancellation: CancellationToken,
    ) -> Result<Run, RunCoordinatorError> {
        if run.step_count >= run.budget.max_steps {
            return self
                .finish(
                    run,
                    RunStatus::LimitExceeded,
                    RunStopReason::StepLimit,
                    None,
                )
                .await;
        }

        let known_call_ids = tool_call_ids(&path);

        let mut attempts = self.store.load_attempts(&run.id).await?;
        let recovering = matches!(
            run.phase,
            RunPhase::CallingAgent | RunPhase::PersistingMessageAndAdvancingSession
        );
        if let Some(interrupted) = attempts
            .last_mut()
            .filter(|attempt| attempt.status == RunAttemptStatus::Running)
        {
            interrupted.status = RunAttemptStatus::Failed;
            interrupted.error = Some(DomainError::transient(
                ErrorCode::RunRecoveryFailed,
                "agent attempt was interrupted before a durable message",
            ));
            interrupted.ended_at = Some(self.clock.now());
            run = self.store.save_attempt(run, interrupted.clone()).await?;
        }

        if run.attempt_count >= run.retry_policy.max_attempts {
            return self
                .finish(
                    run,
                    RunStatus::Failed,
                    RunStopReason::RetryExhausted,
                    Some(DomainError::invariant(
                        ErrorCode::RunRetryExhausted,
                        "agent retry allowance exhausted",
                    )),
                )
                .await;
        }

        let attempt_number = run.attempt_count.saturating_add(1);
        let reason = if recovering {
            RunAttemptReason::Recovery
        } else if attempt_number == 1 {
            RunAttemptReason::Initial
        } else {
            RunAttemptReason::Retry
        };
        let attempt_id = self.ids.attempt_id();
        let mut attempt = RunAttempt {
            id: attempt_id.clone(),
            run_id: run.id.clone(),
            number: attempt_number,
            reason,
            checkpoint_id: if reason == RunAttemptReason::Recovery {
                run.checkpoint_id.clone()
            } else {
                None
            },
            status: RunAttemptStatus::Running,
            error: None,
            started_at: self.clock.now(),
            ended_at: None,
        };
        // Recovery is valid without a compaction checkpoint: the committed Run
        // head itself is the safe checkpoint. Domain recovery attempts require
        // an explicit identity, so persist a synthetic head checkpoint marker.
        if reason == RunAttemptReason::Recovery && attempt.checkpoint_id.is_none() {
            let checkpoint = ait_domain::CheckpointId::new(format!("head:{}", run.step_count));
            attempt.checkpoint_id = Some(checkpoint.clone());
            run.checkpoint_id = Some(checkpoint);
        }
        run.attempt_count = attempt_number;
        run.status = RunStatus::Running;
        run.phase = RunPhase::CallingAgent;
        run = self.store.save_attempt(run, attempt.clone()).await?;

        let response = self
            .controlled(
                self.agent.invoke(AgentInvocation {
                    attempt_id,
                    run_id: run.id.clone(),
                    agent_revision: run.agent_revision,
                    message_path: path,
                    cancellation: cancellation.clone(),
                }),
                &run,
                &cancellation,
            )
            .await;

        match response {
            Controlled::Returned(Ok(response)) => {
                if response.sub_messages.is_empty() {
                    let error = DomainError::invariant(
                        ErrorCode::InvalidRun,
                        "agent returned an empty assistant message",
                    );
                    attempt.status = RunAttemptStatus::Failed;
                    attempt.error = Some(error.clone());
                    attempt.ended_at = Some(self.clock.now());
                    run = self.store.save_attempt(run, attempt).await?;
                    return self
                        .finish(run, RunStatus::Failed, RunStopReason::Failed, Some(error))
                        .await;
                }
                if let Some(call_id) = duplicate_tool_call(&known_call_ids, &response.sub_messages)
                {
                    let error = DomainError::invariant(
                        ErrorCode::ToolCallDuplicate,
                        format!("tool call identity is duplicated in this Run: {call_id}"),
                    );
                    attempt.status = RunAttemptStatus::Failed;
                    attempt.error = Some(error.clone());
                    attempt.ended_at = Some(self.clock.now());
                    run = self.store.save_attempt(run, attempt).await?;
                    return self
                        .finish(run, RunStatus::Failed, RunStopReason::Failed, Some(error))
                        .await;
                }

                let next_step = run.step_count.saturating_add(1);
                let parent = run.last_message_id.unwrap_or(run.base_message_id);
                let message_id = self.ids.message_id();
                let message = Message {
                    id: message_id,
                    project_id: run.project_id.clone(),
                    parent_message_id: Some(parent),
                    role: MessageRole::Assistant,
                    kind: MessageKind::Standard,
                    origin: MessageOrigin::Agent,
                    sub_messages: response.sub_messages,
                    created_by_session_id: run.follow_session_id.clone(),
                    run_id: Some(run.id.clone()),
                    run_seq: Some(next_step),
                    tool_result: None,
                    git_commit: None,
                    metadata: DomainMetadata::default(),
                    created_at: self.clock.now(),
                };
                if let Err(validation) = message.validate() {
                    let error = DomainError::from(validation);
                    attempt.status = RunAttemptStatus::Failed;
                    attempt.error = Some(error.clone());
                    attempt.ended_at = Some(self.clock.now());
                    run = self.store.save_attempt(run, attempt).await?;
                    return self
                        .finish(run, RunStatus::Failed, RunStopReason::Failed, Some(error))
                        .await;
                }
                attempt.status = RunAttemptStatus::Completed;
                attempt.ended_at = Some(self.clock.now());
                run.phase = RunPhase::PersistingMessageAndAdvancingSession;
                run = self.store.save_attempt(run, attempt).await?;
                add_usage(&mut run.usage, &response.usage);
                run.step_count = next_step;
                run.last_message_id = Some(message_id);
                run.phase = RunPhase::AssemblingContext;
                run = self.store.append_message(run, message).await?;

                if let Some(reason) = budget_exceeded(&run) {
                    return self
                        .finish(run, RunStatus::LimitExceeded, reason, None)
                        .await;
                }
                Ok(run)
            }
            Controlled::Returned(Err(error)) => {
                attempt.status =
                    if cancellation.is_cancelled() || error.code == ErrorCode::RunCancelled {
                        RunAttemptStatus::Cancelled
                    } else {
                        RunAttemptStatus::Failed
                    };
                attempt.error = Some(error.clone());
                attempt.ended_at = Some(self.clock.now());
                run = self.store.save_attempt(run, attempt).await?;

                if cancellation.is_cancelled() || error.code == ErrorCode::RunCancelled {
                    return self
                        .finish(
                            run,
                            RunStatus::Cancelled,
                            RunStopReason::Cancelled,
                            Some(error),
                        )
                        .await;
                }
                if error.retryable && run.attempt_count < run.retry_policy.max_attempts {
                    let delay = retry_delay_ms(&run);
                    run.status = RunStatus::RetryWait;
                    run.phase = RunPhase::RetryWait;
                    run.next_retry_at = Some(TimestampMs(self.clock.now().0.saturating_add(delay)));
                    return self.store.save_run(run).await.map_err(Into::into);
                }
                let reason = if error.retryable {
                    RunStopReason::RetryExhausted
                } else {
                    RunStopReason::Failed
                };
                self.finish(run, RunStatus::Failed, reason, Some(error))
                    .await
            }
            Controlled::Cancelled => {
                let error = DomainError::invariant(ErrorCode::RunCancelled, "run was cancelled");
                attempt.status = RunAttemptStatus::Cancelled;
                attempt.error = Some(error.clone());
                attempt.ended_at = Some(self.clock.now());
                run = self.store.save_attempt(run, attempt).await?;
                self.finish(
                    run,
                    RunStatus::Cancelled,
                    RunStopReason::Cancelled,
                    Some(error),
                )
                .await
            }
            Controlled::TimedOut => {
                let error = DomainError::invariant(
                    ErrorCode::RunLimitExceeded,
                    "run runtime limit elapsed during agent invocation",
                );
                attempt.status = RunAttemptStatus::Failed;
                attempt.error = Some(error.clone());
                attempt.ended_at = Some(self.clock.now());
                run = self.store.save_attempt(run, attempt).await?;
                self.finish(
                    run,
                    RunStatus::LimitExceeded,
                    RunStopReason::RuntimeLimit,
                    Some(error),
                )
                .await
            }
        }
    }

    async fn process_tools(
        &self,
        mut run: Run,
        assistant: Message,
        cancellation: CancellationToken,
    ) -> Result<ToolLoop, RunCoordinatorError> {
        let persisted = self
            .store
            .load_tool_executions(&run.id, &assistant.id)
            .await?;

        for (index, part) in assistant.sub_messages.iter().enumerate() {
            let SubMessage::ToolUse(tool_use) = part else {
                continue;
            };
            if let Some(existing) = persisted
                .iter()
                .rev()
                .find(|execution| execution.call_id == tool_use.call_id)
            {
                if existing.tool_result_message_id.is_some() {
                    continue;
                }
                match self
                    .resume_tool(run, existing.clone(), tool_use, cancellation.clone())
                    .await?
                {
                    ToolStep::Continue(next) => run = next,
                    ToolStep::Waiting(next) => return Ok(ToolLoop::Waiting(next)),
                    ToolStep::Terminal(next) => return Ok(ToolLoop::Terminal(next)),
                }
                continue;
            }

            if run.step_count >= run.budget.max_steps {
                run = self
                    .finish(
                        run,
                        RunStatus::LimitExceeded,
                        RunStopReason::StepLimit,
                        None,
                    )
                    .await?;
                return Ok(ToolLoop::Terminal(run));
            }

            let arguments = serde_json::from_str(&tool_use.arguments).map_err(|error| {
                RunCoordinatorError::InvalidState(format!(
                    "persisted tool arguments are invalid: {error}"
                ))
            })?;
            let approval_status = if self
                .tools
                .requires_approval(&tool_use.tool_name, &arguments)
            {
                ToolApprovalStatus::Pending
            } else {
                ToolApprovalStatus::NotRequired
            };
            let execution = ToolExecution {
                id: self.ids.tool_execution_id(),
                run_id: run.id.clone(),
                call_id: tool_use.call_id.clone(),
                assistant_message_id: assistant.id,
                tool_use_index: u32::try_from(index).map_err(|_| {
                    RunCoordinatorError::InvalidState("tool-use index exceeds u32".into())
                })?,
                tool_result_message_id: None,
                tool_name: tool_use.tool_name.clone(),
                arguments,
                attempt: 1,
                approval_status,
                status: ToolExecutionStatus::Pending,
                result: None,
                error: None,
                started_at: None,
                ended_at: None,
                created_at: self.clock.now(),
            };
            run.phase = if approval_status == ToolApprovalStatus::Pending {
                RunPhase::WaitingApproval
            } else {
                RunPhase::ExecutingTool
            };
            run.status = if approval_status == ToolApprovalStatus::Pending {
                RunStatus::WaitingApproval
            } else {
                RunStatus::Running
            };
            run = self
                .store
                .save_tool_execution(run, execution.clone())
                .await?;
            match self
                .resume_tool(run, execution, tool_use, cancellation.clone())
                .await?
            {
                ToolStep::Continue(next) => run = next,
                ToolStep::Waiting(next) => return Ok(ToolLoop::Waiting(next)),
                ToolStep::Terminal(next) => return Ok(ToolLoop::Terminal(next)),
            }
        }
        Ok(ToolLoop::Continue(run))
    }

    #[allow(clippy::too_many_lines)]
    async fn resume_tool(
        &self,
        mut run: Run,
        mut execution: ToolExecution,
        tool_use: &ToolUse,
        cancellation: CancellationToken,
    ) -> Result<ToolStep, RunCoordinatorError> {
        if execution.status.is_terminal() {
            run = self.persist_tool_result(run, execution).await?;
            return Ok(ToolStep::Continue(run));
        }

        if execution.status == ToolExecutionStatus::Running {
            match self
                .controlled(self.tools.reconcile(&execution), &run, &cancellation)
                .await
            {
                Controlled::Returned(Ok(ToolRecovery::Completed(outcome))) => {
                    execution.status = ToolExecutionStatus::Succeeded;
                    execution.result = Some(outcome.output);
                    execution.ended_at = Some(self.clock.now());
                    run.phase = RunPhase::PersistingToolResult;
                    run = self
                        .store
                        .save_tool_execution(run, execution.clone())
                        .await?;
                    run = self.persist_tool_result(run, execution).await?;
                    return Ok(ToolStep::Continue(run));
                }
                Controlled::Returned(Ok(ToolRecovery::RetrySafe)) => {}
                Controlled::Returned(Ok(ToolRecovery::Unknown)) => {
                    let error = DomainError::invariant(
                        ErrorCode::RunRecoveryFailed,
                        format!("tool effect is unknown for call {}", execution.call_id),
                    );
                    run = self
                        .finish(run, RunStatus::Failed, RunStopReason::Failed, Some(error))
                        .await?;
                    return Ok(ToolStep::Terminal(run));
                }
                Controlled::Returned(Err(error)) => {
                    run = self
                        .finish(run, RunStatus::Failed, RunStopReason::Failed, Some(error))
                        .await?;
                    return Ok(ToolStep::Terminal(run));
                }
                Controlled::Cancelled => {
                    run = self
                        .finish(run, RunStatus::Cancelled, RunStopReason::Cancelled, None)
                        .await?;
                    return Ok(ToolStep::Terminal(run));
                }
                Controlled::TimedOut => {
                    run = self
                        .finish(
                            run,
                            RunStatus::LimitExceeded,
                            RunStopReason::RuntimeLimit,
                            None,
                        )
                        .await?;
                    return Ok(ToolStep::Terminal(run));
                }
            }
        }

        if execution.approval_status == ToolApprovalStatus::Pending {
            run.status = RunStatus::WaitingApproval;
            run.phase = RunPhase::WaitingApproval;
            run = self
                .store
                .save_tool_execution(run, execution.clone())
                .await?;
            match self
                .controlled(
                    self.approvals.decide(ApprovalRequest {
                        run_id: run.id.clone(),
                        execution: execution.clone(),
                    }),
                    &run,
                    &cancellation,
                )
                .await
            {
                Controlled::Returned(Ok(ApprovalDecision::Pending)) => {
                    return Ok(ToolStep::Waiting(run));
                }
                Controlled::Returned(Ok(ApprovalDecision::Approved)) => {
                    execution.approval_status = ToolApprovalStatus::Approved;
                    run.status = RunStatus::Running;
                    run.phase = RunPhase::ExecutingTool;
                    run = self
                        .store
                        .save_tool_execution(run, execution.clone())
                        .await?;
                }
                Controlled::Returned(Ok(ApprovalDecision::Denied)) => {
                    execution.approval_status = ToolApprovalStatus::Denied;
                    execution.status = ToolExecutionStatus::Denied;
                    execution.error = Some(DomainError::invariant(
                        ErrorCode::ToolApprovalRequired,
                        "tool approval was denied",
                    ));
                    execution.ended_at = Some(self.clock.now());
                    run.status = RunStatus::Running;
                    run.phase = RunPhase::PersistingToolResult;
                    run = self
                        .store
                        .save_tool_execution(run, execution.clone())
                        .await?;
                    run = self.persist_tool_result(run, execution).await?;
                    return Ok(ToolStep::Continue(run));
                }
                Controlled::Returned(Err(error)) => {
                    run = self
                        .finish(run, RunStatus::Failed, RunStopReason::Failed, Some(error))
                        .await?;
                    return Ok(ToolStep::Terminal(run));
                }
                Controlled::Cancelled => {
                    run = self
                        .finish(run, RunStatus::Cancelled, RunStopReason::Cancelled, None)
                        .await?;
                    return Ok(ToolStep::Terminal(run));
                }
                Controlled::TimedOut => {
                    run = self
                        .finish(
                            run,
                            RunStatus::LimitExceeded,
                            RunStopReason::RuntimeLimit,
                            None,
                        )
                        .await?;
                    return Ok(ToolStep::Terminal(run));
                }
            }
        }

        if cancellation.is_cancelled() {
            execution.status = ToolExecutionStatus::Cancelled;
            execution.error = Some(DomainError::invariant(
                ErrorCode::RunCancelled,
                "run was cancelled before tool dispatch",
            ));
            execution.ended_at = Some(self.clock.now());
        } else {
            execution.status = ToolExecutionStatus::Running;
            if execution.started_at.is_none() {
                execution.started_at = Some(self.clock.now());
            }
            run.status = RunStatus::Running;
            run.phase = RunPhase::ExecutingTool;
            run.usage.tool_executions = run.usage.tool_executions.saturating_add(1);
            run = self
                .store
                .save_tool_execution(run, execution.clone())
                .await?;
            let result = self
                .controlled(
                    self.tools.execute(ToolInvocation {
                        run_id: run.id.clone(),
                        call_id: execution.call_id.clone(),
                        execution_id: execution.id.clone(),
                        tool_name: tool_use.tool_name.clone(),
                        arguments: execution.arguments.clone(),
                        cancellation: cancellation.clone(),
                    }),
                    &run,
                    &cancellation,
                )
                .await;
            let forced_stop = match result {
                Controlled::Returned(Ok(ToolOutcome { output })) => {
                    execution.status = ToolExecutionStatus::Succeeded;
                    execution.result = Some(output);
                    None
                }
                Controlled::Returned(Err(error)) if cancellation.is_cancelled() => {
                    execution.status = ToolExecutionStatus::Cancelled;
                    execution.error = Some(error);
                    Some(RunStopReason::Cancelled)
                }
                Controlled::Returned(Err(error)) => {
                    execution.status = ToolExecutionStatus::Failed;
                    execution.error = Some(error);
                    None
                }
                Controlled::Cancelled => {
                    execution.status = ToolExecutionStatus::Cancelled;
                    execution.error = Some(DomainError::invariant(
                        ErrorCode::RunCancelled,
                        "run was cancelled during tool execution",
                    ));
                    Some(RunStopReason::Cancelled)
                }
                Controlled::TimedOut => {
                    execution.status = ToolExecutionStatus::Cancelled;
                    execution.error = Some(DomainError::invariant(
                        ErrorCode::RunLimitExceeded,
                        "run runtime limit elapsed during tool execution",
                    ));
                    Some(RunStopReason::RuntimeLimit)
                }
            };
            execution.ended_at = Some(self.clock.now());
            run.phase = RunPhase::PersistingToolResult;
            run = self
                .store
                .save_tool_execution(run, execution.clone())
                .await?;
            run = self.persist_tool_result(run, execution).await?;
            if let Some(reason) = forced_stop {
                let status = if reason == RunStopReason::Cancelled {
                    RunStatus::Cancelled
                } else {
                    RunStatus::LimitExceeded
                };
                run = self.finish(run, status, reason, None).await?;
                return Ok(ToolStep::Terminal(run));
            }
            return Ok(ToolStep::Continue(run));
        }

        run.phase = RunPhase::PersistingToolResult;
        run = self
            .store
            .save_tool_execution(run, execution.clone())
            .await?;
        let cancelled = execution.status == ToolExecutionStatus::Cancelled;
        run = self.persist_tool_result(run, execution).await?;
        if cancelled || cancellation.is_cancelled() {
            run = self
                .finish(run, RunStatus::Cancelled, RunStopReason::Cancelled, None)
                .await?;
            Ok(ToolStep::Terminal(run))
        } else {
            Ok(ToolStep::Continue(run))
        }
    }

    async fn persist_tool_result(
        &self,
        mut run: Run,
        mut execution: ToolExecution,
    ) -> Result<Run, RunCoordinatorError> {
        if run.step_count >= run.budget.max_steps {
            return self
                .finish(
                    run,
                    RunStatus::LimitExceeded,
                    RunStopReason::StepLimit,
                    None,
                )
                .await;
        }
        let status = match execution.status {
            ToolExecutionStatus::Succeeded => ToolResultStatus::Succeeded,
            ToolExecutionStatus::Failed => ToolResultStatus::Failed,
            ToolExecutionStatus::Denied => ToolResultStatus::Denied,
            ToolExecutionStatus::Cancelled => ToolResultStatus::Cancelled,
            ToolExecutionStatus::Pending | ToolExecutionStatus::Running => {
                return Err(RunCoordinatorError::InvalidState(
                    "cannot persist a result for a non-terminal tool".into(),
                ));
            }
        };
        let message_id = self.ids.message_id();
        let output = execution
            .result
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .map_err(|error| RunCoordinatorError::InvalidState(error.to_string()))?;
        let error = execution.error.as_ref().map(ToString::to_string);
        run.step_count = run.step_count.saturating_add(1);
        let message = Message {
            id: message_id,
            project_id: run.project_id.clone(),
            parent_message_id: Some(run.last_message_id.unwrap_or(run.base_message_id)),
            role: MessageRole::User,
            kind: MessageKind::ToolResult,
            origin: MessageOrigin::Tool,
            sub_messages: Vec::new(),
            created_by_session_id: run.follow_session_id.clone(),
            run_id: Some(run.id.clone()),
            run_seq: Some(run.step_count),
            tool_result: Some(ToolResult {
                call_id: execution.call_id.clone(),
                status,
                output,
                error,
            }),
            git_commit: None,
            metadata: DomainMetadata::default(),
            created_at: self.clock.now(),
        };
        execution
            .validate_result_message(&message)
            .map_err(|error| {
                RunCoordinatorError::InvalidState(format!("invalid tool result: {error}"))
            })?;
        execution.tool_result_message_id = Some(message_id);
        run.last_message_id = Some(message_id);
        run.status = RunStatus::Running;
        run.phase = RunPhase::AssemblingContext;
        self.store
            .append_tool_result(run, execution, message)
            .await
            .map_err(Into::into)
    }

    async fn settle(&self, mut run: Run) -> Result<Run, RunCoordinatorError> {
        run.status = RunStatus::Settling;
        run.phase = RunPhase::Settling;
        self.store.save_run(run).await.map_err(Into::into)
    }

    async fn finish(
        &self,
        mut run: Run,
        status: RunStatus,
        reason: RunStopReason,
        error: Option<DomainError>,
    ) -> Result<Run, RunCoordinatorError> {
        run.status = status;
        run.phase = RunPhase::Terminal;
        run.stop_reason = Some(reason);
        run.error = error;
        run.next_retry_at = None;
        run.ended_at = Some(self.clock.now());
        run.validate()
            .map_err(|error| RunCoordinatorError::InvalidState(error.to_string()))?;
        self.store.save_run(run).await.map_err(Into::into)
    }

    fn limit_or_cancel(
        &self,
        run: &Run,
        cancellation: &CancellationToken,
    ) -> Option<RunStopReason> {
        if cancellation.is_cancelled() {
            return Some(RunStopReason::Cancelled);
        }
        if let Some(reason) = budget_exceeded(run) {
            return Some(reason);
        }
        if let (Some(started), Some(limit)) = (run.started_at, run.budget.max_runtime)
            && self.clock.now().0.saturating_sub(started.0) >= i64_from_u64(limit.0)
        {
            return Some(RunStopReason::RuntimeLimit);
        }
        None
    }

    fn terminal_outcome(run: Run) -> DriveOutcome {
        if run.status == RunStatus::Completed {
            DriveOutcome::Completed(run)
        } else {
            DriveOutcome::Terminal(run)
        }
    }

    async fn controlled<F, T>(
        &self,
        future: F,
        run: &Run,
        cancellation: &CancellationToken,
    ) -> Controlled<T>
    where
        F: Future<Output = Result<T, DomainError>> + Send,
    {
        if let (Some(started), Some(limit)) = (run.started_at, run.budget.max_runtime) {
            let deadline = TimestampMs(started.0.saturating_add(i64_from_u64(limit.0)));
            tokio::select! {
                biased;
                result = future => Controlled::Returned(result),
                () = cancellation.cancelled() => Controlled::Cancelled,
                () = self.clock.sleep_until(deadline) => Controlled::TimedOut,
            }
        } else {
            tokio::select! {
                biased;
                result = future => Controlled::Returned(result),
                () = cancellation.cancelled() => Controlled::Cancelled,
            }
        }
    }
}

enum Controlled<T> {
    Returned(Result<T, DomainError>),
    Cancelled,
    TimedOut,
}

enum ToolLoop {
    Continue(Run),
    Waiting(Run),
    Terminal(Run),
}

enum ToolStep {
    Continue(Run),
    Waiting(Run),
    Terminal(Run),
}

fn visible_tail(path: &[ProjectedMessage]) -> Option<&Message> {
    path.iter().rev().find_map(|message| match message {
        ProjectedMessage::Visible(message) => Some(message),
        ProjectedMessage::Redacted { .. } => None,
    })
}

fn pending_tool_assistant(path: &[ProjectedMessage]) -> Option<&Message> {
    let resolved: HashSet<&str> = path
        .iter()
        .filter_map(|projected| match projected {
            ProjectedMessage::Visible(Message {
                tool_result: Some(result),
                ..
            }) => Some(result.call_id.as_str()),
            _ => None,
        })
        .collect();
    path.iter().rev().find_map(|projected| match projected {
        ProjectedMessage::Visible(message)
            if message.role == MessageRole::Assistant
                && message.sub_messages.iter().any(|part| {
                    matches!(part, SubMessage::ToolUse(tool_use) if !resolved.contains(tool_use.call_id.as_str()))
                }) =>
        {
            Some(message)
        }
        _ => None,
    })
}

fn tool_call_ids(path: &[ProjectedMessage]) -> HashSet<String> {
    path.iter()
        .filter_map(|projected| match projected {
            ProjectedMessage::Visible(message) => Some(&message.sub_messages),
            ProjectedMessage::Redacted { .. } => None,
        })
        .flatten()
        .filter_map(|part| match part {
            SubMessage::ToolUse(tool_use) => Some(tool_use.call_id.clone()),
            _ => None,
        })
        .collect()
}

fn duplicate_tool_call(known: &HashSet<String>, proposed: &[SubMessage]) -> Option<String> {
    let mut seen = known.clone();
    proposed.iter().find_map(|part| match part {
        SubMessage::ToolUse(tool_use) if !seen.insert(tool_use.call_id.clone()) => {
            Some(tool_use.call_id.clone())
        }
        _ => None,
    })
}

fn add_usage(total: &mut RunUsage, delta: &RunUsage) {
    total.input_tokens = total.input_tokens.saturating_add(delta.input_tokens);
    total.cached_input_tokens = total
        .cached_input_tokens
        .saturating_add(delta.cached_input_tokens);
    total.output_tokens = total.output_tokens.saturating_add(delta.output_tokens);
    total.tool_executions = total.tool_executions.saturating_add(delta.tool_executions);
    total.cost = match (total.cost, delta.cost) {
        (None, None) => None,
        (left, right) => Some(CostMicros(
            left.map_or(0, |cost| cost.0)
                .saturating_add(right.map_or(0, |cost| cost.0)),
        )),
    };
}

fn budget_exceeded(run: &Run) -> Option<RunStopReason> {
    if run
        .budget
        .token_budget
        .is_some_and(|budget| run.usage.total_tokens() >= budget)
    {
        return Some(RunStopReason::TokenBudget);
    }
    if run
        .budget
        .cost_budget
        .is_some_and(|budget| run.usage.cost.is_some_and(|cost| cost >= budget))
    {
        return Some(RunStopReason::CostBudget);
    }
    None
}

fn retry_delay_ms(run: &Run) -> i64 {
    let exponent = run.attempt_count.saturating_sub(1).min(31);
    let multiplier = 1_u64 << exponent;
    let delay = run
        .retry_policy
        .initial_delay
        .0
        .saturating_mul(multiplier)
        .min(run.retry_policy.max_delay.0);
    i64_from_u64(delay)
}

fn i64_from_u64(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

/// Production wall clock backed by `SystemTime` and Tokio timers.
#[derive(Debug, Default)]
pub struct SystemClock;

#[async_trait::async_trait]
impl RunClock for SystemClock {
    fn now(&self) -> TimestampMs {
        let millis = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_or(0, |duration| duration.as_millis());
        TimestampMs(i64::try_from(millis).unwrap_or(i64::MAX))
    }

    async fn sleep_until(&self, deadline: TimestampMs) {
        let remaining = deadline.0.saturating_sub(self.now().0);
        if remaining > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(
                u64::try_from(remaining).unwrap_or(u64::MAX),
            ))
            .await;
        }
    }
}

/// UUID-backed production identity generator.
#[derive(Debug, Default)]
pub struct UuidIds;

impl RunIdGenerator for UuidIds {
    fn message_id(&self) -> MessageId {
        MessageId::new(Uuid::new_v4())
    }

    fn attempt_id(&self) -> ait_domain::RunAttemptId {
        ait_domain::RunAttemptId::new(Uuid::new_v4().to_string())
    }

    fn tool_execution_id(&self) -> ait_domain::ToolExecutionId {
        ait_domain::ToolExecutionId::new(Uuid::new_v4().to_string())
    }
}
