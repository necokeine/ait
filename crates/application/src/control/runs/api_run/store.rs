//! Durable `RunStore` bridge between the coordinator and application records.
use std::sync::Mutex;

use ait_contracts::{
    ApiError,
    sensitive::{validate_serialized_tool_arguments, validate_tool_argument_value},
};
use ait_domain::{
    DomainError, ErrorCode, LifecycleStatus, Message, MessageId, ProjectedMessage, Run, RunAttempt,
    RunId, RunStatus, SubMessage, ToolExecution,
};
use ait_ports::{
    ApprovalDecision, CompletionResult, ControlStoreError, RunStore, RunStoreError, ToolInvocation,
    ToolOutcome, ToolRecovery, ToolUsageRecorder,
};
use async_trait::async_trait;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::control::LocalControlService;
use crate::control::conversation::{MessageRecord, release_session};
use crate::control::errors::error;
use crate::control::events::{now, pending};
use crate::control::persistence::{HasMessages, HasSessions};
use crate::control::runs::{ApiRunExecution, RunRecord, is_terminal_run_status};

use super::terminal::{append_terminal_results, interrupt};

pub(super) fn store_failure(_: impl std::fmt::Debug) -> RunStoreError {
    RunStoreError::Other("host Run persistence failed".into())
}

fn invalid(message: &str) -> ApiError {
    error(ErrorCode::InvalidRun, message, false)
}

fn conflict() -> RunStoreError {
    RunStoreError::Conflict("host Run state changed".into())
}

fn sensitive_tool_input() -> RunStoreError {
    RunStoreError::Other("host Run rejected sensitive tool input".into())
}

fn validate_message_tool_inputs(message: &Message) -> Result<(), RunStoreError> {
    for part in &message.sub_messages {
        if let SubMessage::ToolUse(tool) = part {
            validate_serialized_tool_arguments(&tool.arguments)
                .map_err(|_| sensitive_tool_input())?;
        }
    }
    Ok(())
}

fn validate_execution_tool_input(tool: &ToolExecution) -> Result<(), RunStoreError> {
    validate_tool_argument_value(&tool.arguments).map_err(|_| sensitive_tool_input())
}

fn message_id(id: &str) -> Result<MessageId, RunStoreError> {
    Uuid::parse_str(id)
        .map(MessageId::new)
        .map_err(store_failure)
}

pub(super) fn status(run: &Run) -> ait_domain::LifecycleStatus {
    run.status.into()
}

pub(in crate::control) struct ControlRunStore {
    pub(in crate::control) service: LocalControlService,
    pub(in crate::control) worker_deadline: std::sync::atomic::AtomicI64,
    worker_connection: Mutex<Option<WorkerConnection>>,
    records: crate::control::persistence::access::RecordAccess,
    id: String,
    expected: Mutex<Option<Run>>,
}
struct WorkerConnection {
    lease: ait_ports::WorkerLease,
    closed: tokio_util::sync::CancellationToken,
}
impl Drop for ControlRunStore {
    fn drop(&mut self) {
        if let Ok(Some(connection)) = self.worker_connection.get_mut() {
            connection.closed.cancel();
        }
    }
}
struct WorkerOperation<'a> {
    lease: &'a ait_ports::WorkerLease,
    id: &'a str,
    fingerprint: String,
    completed: Option<bool>,
}
fn validate_worker_transition(
    before: &Run,
    after: &Run,
    completion: Option<bool>,
) -> Result<(), RunStoreError> {
    before
        .validate_worker_successor(after, completion)
        .map_err(|_| conflict())
}

impl ControlRunStore {
    async fn validate_project_owner(
        &self,
        lease: &ait_ports::WorkerLease,
    ) -> Result<(), RunStoreError> {
        let loaded = self
            .records
            .read_run_view_records(&self.id)
            .await
            .map_err(store_failure)?;
        if let Some(owner) = &lease.project_owner {
            if loaded
                .version
                .projects
                .get(&owner.project_id)
                .map(|version| version.owner(&owner.project_id))
                .as_ref()
                != Some(owner)
            {
                return Err(conflict());
            }
        } else if !loaded.version.projects.is_empty() {
            return Err(conflict());
        }
        Ok(())
    }
    pub(super) fn new(service: LocalControlService, id: String) -> Self {
        let records = service.records();
        Self {
            service,
            worker_deadline: std::sync::atomic::AtomicI64::new(i64::MAX),
            worker_connection: Mutex::new(None),
            records,
            id,
            expected: Mutex::new(None),
        }
    }

    pub(in crate::control) fn live_connection(
        &self,
        lease: &ait_ports::WorkerLease,
    ) -> Option<tokio_util::sync::CancellationToken> {
        self.worker_connection
            .lock()
            .ok()?
            .as_ref()
            .filter(|connection| connection.lease == *lease && !connection.closed.is_cancelled())
            .map(|connection| connection.closed.clone())
    }
    pub(in crate::control) fn worker_deadline(&self) -> i64 {
        self.worker_deadline
            .load(std::sync::atomic::Ordering::SeqCst)
    }
    pub(super) async fn latest_view(&self) -> Result<RunRecord, ApiError> {
        self.records
            .read_run_view_records(&self.id)
            .await?
            .original
            .runs
            .into_iter()
            .find(|r| r.id == self.id)
            .ok_or_else(|| invalid("Run disappeared"))
    }

    pub(super) async fn initialize(&self, view: &RunRecord) -> Result<(), RunStoreError> {
        for _ in 0..8 {
            let loaded = self
                .records
                .read_run_view_records(&self.id)
                .await
                .map_err(store_failure)?;
            let mut state = loaded.original.clone();
            let target = state
                .runs
                .iter_mut()
                .find(|r| r.id == self.id)
                .ok_or_else(conflict)?;
            if target.execution().is_some() || is_terminal_run_status(target.status()) {
                return Ok(());
            }
            let now = ait_domain::TimestampMs(now());
            let run = Run {
                id: RunId::new(&view.id),
                project_id: ait_domain::ProjectId::new(&view.project_id),
                base_message_id: message_id(&view.base_message_id)?,
                last_message_id: None,
                follow_session_id: view.session_id.as_ref().map(ait_domain::SessionId::new),
                agent_id: ait_domain::AgentId::new(&view.agent_id),
                agent_revision: view.agent_revision,
                agent_snapshot: ait_domain::AgentConfigSnapshot {
                    agent_id: ait_domain::AgentId::new(&view.agent_id),
                    revision: view.agent_revision,
                    driver_type: format!("{:?}", view.provider.kind),
                    connection_name: view.config.provider_id.clone(),
                    model: view.config.model.clone(),
                    endpoint: view.provider.url.clone(),
                    capabilities: std::collections::BTreeSet::default(),
                    default_parameters: ait_domain::DomainMetadata::default(),
                    tool_policy: ait_domain::ToolPolicy::default(),
                    config_digest: format!(
                        "{:x}",
                        Sha256::digest(serde_json::to_vec(&view.config).map_err(store_failure)?)
                    ),
                },
                trigger: view.trigger,
                cron_id: view.cron_id.as_ref().map(ait_domain::CronId::new),
                scheduled_at: view.scheduled_at.map(ait_domain::TimestampMs),
                status: RunStatus::Queued,
                phase: ait_domain::RunPhase::Queued,
                stop_reason: None,
                error: None,
                step_count: 0,
                budget: ait_domain::RunBudget {
                    max_steps: 128,
                    token_budget: Some(1_000_000),
                    cost_budget: None,
                    max_runtime: Some(ait_domain::DurationMs(300_000)),
                },
                usage: ait_domain::RunUsage::default(),
                attempt_count: 0,
                compaction_count: 0,
                retry_policy: ait_domain::RetryPolicy {
                    max_attempts: 3,
                    initial_delay: ait_domain::DurationMs(100),
                    max_delay: ait_domain::DurationMs(1000),
                },
                next_retry_at: None,
                checkpoint_id: None,
                queue_version: 0,
                queue_cursor: 0,
                dedupe_key: None,
                started_at: None,
                ended_at: None,
                created_at: now,
            };
            run.validate().map_err(store_failure)?;
            target.install_execution(ApiRunExecution {
                run,
                attempts: Vec::new(),
                tools: Vec::new(),
                worker_instance_id: None,
                worker_receipts: std::collections::BTreeMap::new(),
            });
            match self
                .records
                .persist_records(&loaded, &state, Vec::new())
                .await
            {
                Ok(()) => return Ok(()),
                Err(ControlStoreError::Conflict) => {}
                Err(e) => return Err(store_failure(e)),
            }
        }
        Err(conflict())
    }
    async fn state(&self) -> Result<ApiRunExecution, RunStoreError> {
        self.records
            .read_run_view_records(&self.id)
            .await
            .map_err(store_failure)?
            .original
            .runs
            .into_iter()
            .find(|r| r.id == self.id)
            .and_then(|r| r.execution().cloned())
            .ok_or_else(conflict)
    }
    #[allow(
        clippy::too_many_lines,
        reason = "message, Session, canonical Run and receipt are one atomic transaction"
    )]
    async fn persist(
        &self,
        run: Run,
        attempt: Option<RunAttempt>,
        tool: Option<ToolExecution>,
        message: Option<Message>,
        operation: Option<WorkerOperation<'_>>,
    ) -> Result<Run, RunStoreError> {
        run.validate().map_err(store_failure)?;
        let expected = self
            .expected
            .lock()
            .map_err(store_failure)?
            .clone()
            .ok_or_else(conflict)?;
        for _ in 0..8 {
            let mut run = run.clone();
            let loaded = self
                .records
                .read_api_run_records(&self.id)
                .await
                .map_err(store_failure)?;
            let mut state = loaded.original.clone();
            let index = state
                .runs
                .iter()
                .position(|r| r.id == self.id)
                .ok_or_else(conflict)?;
            let mut view = state.runs[index].clone();
            if let Some(operation) = &operation {
                if loaded
                    .version
                    .projects
                    .get(&view.project_id)
                    .map(|version| version.owner(&view.project_id))
                    != operation.lease.project_owner
                {
                    return Err(conflict());
                }
                if view.id != operation.lease.run_id.as_str()
                    || view.lease_epoch != operation.lease.epoch
                    || view
                        .execution()
                        .and_then(|e| e.worker_instance_id.as_deref())
                        != Some(&operation.lease.instance_id)
                {
                    return Err(conflict());
                }
                if let Some(receipt) = view
                    .execution()
                    .and_then(|e| e.worker_receipts.get(operation.id))
                {
                    if receipt.fingerprint != operation.fingerprint {
                        return Err(conflict());
                    }
                    return Ok(receipt.run.clone());
                }
                validate_worker_transition(&expected, &run, operation.completed)?;
                if message.as_ref().map_or(
                    run.last_message_id != expected.last_message_id
                        || run.step_count != expected.step_count,
                    |message| run.last_message_id != Some(message.id),
                ) || (attempt.is_none() && run.attempt_count != expected.attempt_count)
                {
                    return Err(conflict());
                }
            }
            if view.status() == LifecycleStatus::Cancelling && run.status.is_terminal() {
                run.status = RunStatus::Cancelled;
                run.stop_reason = Some(ait_domain::RunStopReason::Cancelled);
            }
            let execution = view.execution_mut().ok_or_else(conflict)?;
            if execution.run != expected {
                return Err(conflict());
            }
            if let Some(attempt) = &attempt {
                attempt.validate().map_err(store_failure)?;
                if attempt.run_id != run.id || attempt.number > run.attempt_count {
                    return Err(conflict());
                }
                execution.attempts.retain(|a| a.id != attempt.id);
                execution.attempts.push(attempt.clone());
            }
            if let Some(tool) = &tool {
                tool.validate().map_err(store_failure)?;
                validate_tool_child(&state, &run, tool, message.as_ref())?;
                if execution
                    .tools
                    .iter()
                    .any(|t| t.id != tool.id && t.call_id == tool.call_id)
                {
                    return Err(conflict());
                }
                execution.tools.retain(|t| t.id != tool.id);
                execution.tools.push(tool.clone());
            }
            execution.run = run.clone();
            if let Some(message) = &message {
                append_projection(&mut state, &view, &run, &expected, message)?;
            }
            if run.status.is_terminal() {
                crate::control::tool_approvals::expire(
                    &mut view,
                    if run.status == RunStatus::Cancelled {
                        ait_domain::ToolApprovalState::Cancelled
                    } else {
                        ait_domain::ToolApprovalState::Expired
                    },
                );
                if run.status != RunStatus::Completed {
                    interrupt(&mut view);
                    append_terminal_results(&mut state, &mut view)?;
                    run = view.execution().ok_or_else(conflict)?.run.clone();
                }
                release_session(&mut state, &view);
            }
            state.runs[index] = view;
            if let Some(operation) = &operation {
                let execution = state.runs[index].execution_mut().ok_or_else(conflict)?;
                if execution.worker_receipts.len() >= 8192 {
                    return Err(conflict());
                }
                execution.worker_receipts.insert(
                    operation.id.into(),
                    ait_contracts::WorkerCommitReceipt {
                        fingerprint: operation.fingerprint.clone(),
                        run: run.clone(),
                        completed: operation.completed,
                    },
                );
            }
            // Run events retain the public shape; pending() excludes execution payloads.
            let events = vec![pending(
                "run.updated",
                Some(self.id.clone()),
                &state.runs[index],
            )];
            match self.records.persist_records(&loaded, &state, events).await {
                Ok(()) => {
                    *self.expected.lock().map_err(store_failure)? = Some(run.clone());
                    return Ok(run);
                }
                Err(ControlStoreError::Conflict) => {}
                Err(e) => return Err(store_failure(e)),
            }
        }
        Err(conflict())
    }
}
#[async_trait]
impl RunStore for ControlRunStore {
    async fn register_worker_process(
        &self,
        lease: &ait_ports::WorkerLease,
        pid: u32,
    ) -> Result<(), RunStoreError> {
        self.validate_project_owner(lease).await?;
        if let Some(owner) = &lease.project_owner {
            self.records
                .store
                .register_worker_process(owner, pid)
                .await
                .map_err(store_failure)?;
        }
        Ok(())
    }
    async fn release_worker_process(
        &self,
        lease: &ait_ports::WorkerLease,
        pid: u32,
    ) -> Result<(), RunStoreError> {
        if let Some(owner) = &lease.project_owner {
            self.records
                .store
                .release_worker_process(owner, pid)
                .await
                .map_err(store_failure)?;
        }
        Ok(())
    }
    async fn request_tool_interaction(
        &self,
        lease: &ait_ports::WorkerLease,
        call_id: String,
        execution_id: String,
        tool_name: String,
        arguments: Value,
    ) -> Result<ToolOutcome, DomainError> {
        let Some(connection) = self.live_connection(lease) else {
            return Err(DomainError::invariant(
                ErrorCode::RunRecoveryFailed,
                "worker connection is unavailable",
            ));
        };
        self.service
            .request_api_tool_interaction(
                ToolInvocation {
                    run_id: lease.run_id.clone(),
                    call_id,
                    execution_id: ait_domain::ToolExecutionId::new(execution_id),
                    tool_name,
                    arguments,
                    usage: ToolUsageRecorder::default(),
                    cancellation: connection.child_token(),
                },
                Some(lease),
                self.worker_deadline(),
                connection,
            )
            .await
            .map_err(crate::control::errors::api_domain_error)
    }
    async fn recover_tool_interaction(
        &self,
        execution: &ToolExecution,
    ) -> Result<ToolRecovery, DomainError> {
        self.service
            .recover_api_tool_interaction(execution)
            .await
            .map_err(crate::control::errors::api_domain_error)
    }
    async fn request_tool_approval(
        &self,
        lease: &ait_ports::WorkerLease,
        execution: ToolExecution,
    ) -> Result<ApprovalDecision, DomainError> {
        let Some(connection) = self.live_connection(lease) else {
            return Ok(ApprovalDecision::Denied);
        };
        self.service
            .request_api_tool_approval(
                execution,
                Some(lease),
                self.worker_deadline
                    .load(std::sync::atomic::Ordering::SeqCst),
                connection,
            )
            .await
            .map_err(crate::control::errors::api_domain_error)
    }
    async fn consume_tool_grant(
        &self,
        lease: &ait_ports::WorkerLease,
        grant: &ait_domain::ToolGrant,
    ) -> Result<bool, DomainError> {
        let Some(connection) = self.live_connection(lease) else {
            return Ok(false);
        };
        self.service
            .consume_api_tool_grant(grant, Some(lease), &connection)
            .await
            .map_err(crate::control::errors::api_domain_error)
    }
    fn interrupt_tool_approvals(&self, lease: &ait_ports::WorkerLease) {
        if let Ok(connection) = self.worker_connection.lock()
            && let Some(connection) = connection
                .as_ref()
                .filter(|connection| connection.lease == *lease)
        {
            connection.closed.cancel();
        }
        self.service
            .interrupt_api_tool_approvals(lease.run_id.as_str(), lease.epoch);
        self.service
            .interrupt_tool_interactions(lease.run_id.as_str(), lease.epoch);
    }
    fn set_worker_deadline(&self, deadline: i64) {
        self.worker_deadline
            .store(deadline, std::sync::atomic::Ordering::SeqCst);
    }
    async fn claim_worker(&self, instance: &str) -> Result<ait_ports::WorkerLease, RunStoreError> {
        for _ in 0..8 {
            let loaded = self
                .records
                .read_run_view_records(&self.id)
                .await
                .map_err(store_failure)?;
            let mut state = loaded.original.clone();
            let view = state
                .runs
                .iter_mut()
                .find(|v| v.id == self.id)
                .ok_or_else(conflict)?;
            if is_terminal_run_status(view.status()) {
                return Err(conflict());
            }
            view.lease_epoch = view.lease_epoch.checked_add(1).ok_or_else(conflict)?;
            crate::control::tool_approvals::expire(view, ait_domain::ToolApprovalState::Expired);
            crate::control::tool_approvals::fence_pending_tools(view);
            view.execution_mut()
                .ok_or_else(conflict)?
                .worker_instance_id = Some(instance.into());
            let lease = ait_ports::WorkerLease {
                project_owner: loaded
                    .version
                    .projects
                    .get(&view.project_id)
                    .map(|version| version.owner(&view.project_id)),
                run_id: RunId::new(&self.id),
                instance_id: instance.into(),
                epoch: view.lease_epoch,
            };
            match self
                .records
                .persist_records(&loaded, &state, Vec::new())
                .await
            {
                Ok(()) => {
                    let mut connection = self.worker_connection.lock().map_err(store_failure)?;
                    if connection
                        .as_ref()
                        .is_some_and(|connection| connection.lease.epoch >= lease.epoch)
                    {
                        return Err(conflict());
                    }
                    if let Some(previous) = connection.replace(WorkerConnection {
                        lease: lease.clone(),
                        closed: tokio_util::sync::CancellationToken::new(),
                    }) {
                        previous.closed.cancel();
                    }
                    return Ok(lease);
                }
                Err(ControlStoreError::Conflict) => {}
                Err(e) => return Err(store_failure(e)),
            }
        }
        Err(conflict())
    }
    async fn commit_worker(
        &self,
        lease: &ait_ports::WorkerLease,
        operation_id: &str,
        mutation: ait_ports::RunMutation,
    ) -> Result<ait_ports::RunReceipt, RunStoreError> {
        use ait_ports::RunMutation;
        self.validate_project_owner(lease).await?;
        if operation_id.is_empty() || operation_id.len() > 256 || lease.run_id.as_str() != self.id {
            return Err(conflict());
        }
        let fingerprint = format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(&mutation).map_err(store_failure)?)
        );
        let view = self.latest_view().await.map_err(store_failure)?;
        let execution = view.execution().ok_or_else(conflict)?;
        if view.lease_epoch != lease.epoch
            || execution.worker_instance_id.as_deref() != Some(&lease.instance_id)
        {
            return Err(conflict());
        }
        if let Some(receipt) = execution.worker_receipts.get(operation_id) {
            if receipt.fingerprint != fingerprint {
                return Err(conflict());
            }
            return Ok(ait_ports::RunReceipt {
                run: receipt.run.clone(),
                completed: receipt.completed,
            });
        }
        let (run, attempt, tool, message, completed) = match mutation {
            RunMutation::SaveRun(run) => (run, None, None, None, None),
            RunMutation::SaveAttempt(run, attempt) => (run, Some(attempt), None, None, None),
            RunMutation::AppendMessage(run, message) => (run, None, None, Some(message), None),
            RunMutation::SaveTool(run, tool) => (run, None, Some(tool), None, None),
            RunMutation::AppendToolResult(run, tool, message) => {
                (run, None, Some(tool), Some(message), None)
            }
            RunMutation::Complete(run, version) => {
                if execution
                    .tools
                    .iter()
                    .any(|t| !t.status.is_terminal() || t.tool_result_message_id.is_none())
                {
                    return Err(conflict());
                }
                if execution.run.queue_version == version {
                    (run, None, None, None, Some(true))
                } else {
                    (execution.run.clone(), None, None, None, Some(false))
                }
            }
            RunMutation::DrainQueue(run) => {
                if run.queue_version != run.queue_cursor {
                    return Err(conflict());
                }
                (run, None, None, None, None)
            }
        };
        let run = self
            .persist(
                run,
                attempt,
                tool,
                message,
                Some(WorkerOperation {
                    lease,
                    id: operation_id,
                    fingerprint,
                    completed,
                }),
            )
            .await?;
        Ok(ait_ports::RunReceipt { run, completed })
    }
    async fn load_run(&self, _: &RunId) -> Result<Run, RunStoreError> {
        let run = self.state().await?.run;
        *self.expected.lock().map_err(store_failure)? = Some(run.clone());
        Ok(run)
    }
    async fn load_message_path(
        &self,
        head: &MessageId,
    ) -> Result<Vec<ProjectedMessage>, RunStoreError> {
        let state = self
            .records
            .read_message_path_records(&head.as_uuid().to_string())
            .await
            .map_err(store_failure)?
            .original;
        crate::control::conversation::domain_path(&state.messages, &head.to_string())
            .map(|messages| {
                messages
                    .into_iter()
                    .map(ProjectedMessage::Visible)
                    .collect()
            })
            .map_err(store_failure)
    }

    async fn load_attempts(&self, _: &RunId) -> Result<Vec<RunAttempt>, RunStoreError> {
        Ok(self.state().await?.attempts)
    }
    async fn load_tool_executions(
        &self,
        _: &RunId,
        assistant: &MessageId,
    ) -> Result<Vec<ToolExecution>, RunStoreError> {
        Ok(self
            .state()
            .await?
            .tools
            .into_iter()
            .filter(|t| &t.assistant_message_id == assistant)
            .collect())
    }
    async fn save_run(&self, run: Run) -> Result<Run, RunStoreError> {
        self.persist(run, None, None, None, None).await
    }
    async fn save_attempt(&self, run: Run, attempt: RunAttempt) -> Result<Run, RunStoreError> {
        self.persist(run, Some(attempt), None, None, None).await
    }
    async fn append_message(&self, run: Run, message: Message) -> Result<Run, RunStoreError> {
        self.persist(run, None, None, Some(message), None).await
    }
    async fn save_tool_execution(
        &self,
        run: Run,
        tool: ToolExecution,
    ) -> Result<Run, RunStoreError> {
        self.persist(run, None, Some(tool), None, None).await
    }
    async fn append_tool_result(
        &self,
        run: Run,
        tool: ToolExecution,
        message: Message,
    ) -> Result<Run, RunStoreError> {
        self.persist(run, None, Some(tool), Some(message), None)
            .await
    }
    async fn try_complete(
        &self,
        run: Run,
        version: u64,
    ) -> Result<CompletionResult, RunStoreError> {
        let state = self.state().await?;
        if state.run.queue_version != version {
            return Ok(CompletionResult::QueueChanged(state.run));
        }
        if state
            .tools
            .iter()
            .any(|t| !t.status.is_terminal() || t.tool_result_message_id.is_none())
        {
            return Err(conflict());
        }
        Ok(CompletionResult::Completed(self.save_run(run).await?))
    }
    async fn drain_queue(&self, run: Run) -> Result<Run, RunStoreError> {
        if run.queue_version != run.queue_cursor {
            return Err(conflict());
        }
        Ok(run)
    }
}

pub(super) fn append_projection(
    state: &mut (impl HasMessages + HasSessions),
    view: &RunRecord,
    run: &Run,
    expected: &Run,
    message: &Message,
) -> Result<(), RunStoreError> {
    validate_message_tool_inputs(message)?;
    message.validate().map_err(store_failure)?;
    if message.project_id != run.project_id
        || message.run_id.as_ref() != Some(&run.id)
        || message.run_seq != Some(run.step_count)
        || run.step_count != expected.step_count + 1
        || message.parent_message_id
            != Some(expected.last_message_id.unwrap_or(expected.base_message_id))
        || state
            .messages()
            .iter()
            .any(|m| m.id == message.id.as_uuid().to_string())
    {
        return Err(conflict());
    }
    if let Some(result) = &message.tool_result
        && state.messages().iter().any(|m| {
            m.data.as_ref().is_some_and(|data| {
                data["native_message"]["tool_result"]["call_id"] == result.call_id
                    && data["native_message"]["run_id"] == run.id.as_str()
            })
        })
    {
        return Err(conflict());
    }
    let mut message_state = MessageRecord::from(message);
    message_state
        .data
        .as_mut()
        .and_then(Value::as_object_mut)
        .expect("domain Message projection is an object")
        .insert("agent_revision".into(), json!(run.agent_revision));
    state.messages_mut().push(message_state);
    if let Some(session) = state
        .sessions_mut()
        .iter_mut()
        .find(|s| Some(&s.id) == view.session_id.as_ref())
    {
        let owns_pointer = session.active_run_id() == Some(&view.id)
            || (run.status.is_terminal() && session.active_run_id().is_none());
        if !owns_pointer
            || session.current_message_id()
                != expected
                    .last_message_id
                    .unwrap_or(expected.base_message_id)
                    .as_uuid()
                    .to_string()
        {
            // Old terminal projections may already have released this Session.
            // Preserve a moved/rebound pointer; the result remains on its Run's
            // immutable branch and is still independently queryable.
            if run.status.is_terminal() {
                return Ok(());
            }
            return Err(conflict());
        }
        session
            .reference
            .advance(
                session.reference.head(),
                session.reference.version(),
                message.parent_message_id,
                message.id,
            )
            .map_err(store_failure)?;
    }

    Ok(())
}

fn validate_tool_child(
    state: &impl HasMessages,
    run: &Run,
    tool: &ToolExecution,
    result: Option<&Message>,
) -> Result<(), RunStoreError> {
    validate_execution_tool_input(tool)?;
    if tool.run_id != run.id {
        return Err(conflict());
    }
    let native = state
        .messages()
        .iter()
        .find(|m| m.id == tool.assistant_message_id.as_uuid().to_string())
        .and_then(|m| m.data.as_ref())
        .and_then(|d| d.get("native_message"))
        .ok_or_else(conflict)?;
    let assistant: Message = serde_json::from_value(native.clone()).map_err(store_failure)?;
    let Some(SubMessage::ToolUse(proposal)) =
        assistant.sub_messages.get(tool.tool_use_index as usize)
    else {
        return Err(conflict());
    };
    if assistant.run_id.as_ref() != Some(&run.id)
        || proposal.call_id != tool.call_id
        || proposal.tool_name != tool.tool_name
        || serde_json::from_str::<Value>(&proposal.arguments).map_err(store_failure)?
            != tool.arguments
    {
        return Err(conflict());
    }
    if let Some(result) = result {
        if tool.tool_result_message_id != Some(result.id) {
            return Err(conflict());
        }
        let mut unlinked = tool.clone();
        unlinked.tool_result_message_id = None;
        unlinked
            .validate_result_message(result)
            .map_err(store_failure)?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "private_input_tests.rs"]
mod private_input_tests;
