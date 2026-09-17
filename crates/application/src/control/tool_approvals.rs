//! Application-owned API tool authorization, durable decisions and one-use grants.
use super::errors::{api_domain_error, error, store_error};
use super::events::{now, pending};
use super::project::worktrees::run_workdir;
use super::{LocalControlService, model::RunState};
use ait_contracts::{ApiError, Command, CommandResult, RunView, ToolApprovalAction};
use ait_domain::{
    DomainError, ErrorCode, LifecycleStatus, ToolApprovalRecord, ToolApprovalState as Status,
    ToolApprovalStatus, ToolExecution, ToolExecutionStatus, ToolGrant,
};
use ait_ports::{ApprovalDecision, ApprovalRequest, ControlStoreError, RunApproval, WorkerLease};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio_util::sync::CancellationToken;

pub(super) struct ToolApprovalWaiter {
    run_id: String,
    epoch: u64,
    connection: CancellationToken,
    sender: tokio::sync::watch::Sender<Option<Status>>,
}
struct WaitGuard {
    registry: Arc<Mutex<HashMap<String, ToolApprovalWaiter>>>,
    id: String,
}
impl Drop for WaitGuard {
    fn drop(&mut self) {
        if let Ok(mut waiters) = self.registry.lock() {
            waiters.remove(&self.id);
        }
    }
}
fn invalid() -> ApiError {
    error(
        ErrorCode::ToolApprovalRequired,
        "tool approval is invalid, expired, or no longer pending",
        false,
    )
}
fn digest(tool: &ToolExecution) -> Result<String, ApiError> {
    ait_contracts::sensitive::validate_tool_argument_value(&tool.arguments)
        .map_err(|_| invalid())?;
    Ok(format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&tool.arguments).map_err(|_| invalid())?)
    ))
}
fn active(run: &RunState, lease: Option<&WorkerLease>) -> bool {
    !run.status().is_terminal()
        && run.status() != LifecycleStatus::Cancelling
        && lease.is_none_or(|lease| {
            run.id == lease.run_id.as_str()
                && run.lease_epoch == lease.epoch
                && run
                    .execution()
                    .and_then(|e| e.worker_instance_id.as_deref())
                    == Some(lease.instance_id.as_str())
        })
}
fn tool_for<'a>(run: &'a RunState, grant: &ToolGrant) -> Option<&'a ToolExecution> {
    run.execution()?.tools.iter().find(|t| {
        t.id.as_str() == grant.execution_id
            && t.call_id == grant.call_id
            && t.run_id.as_str() == grant.run_id
            && t.tool_name == grant.target.tool_name
    })
}

/// Revoke all outstanding authority at cancellation, terminal settlement, or lease change.
pub(super) fn expire(run: &mut RunState, status: Status) {
    for approval in &mut run.tool_approvals {
        if matches!(approval.status, Status::Pending | Status::Approved) {
            approval.status = status;
            approval.decided_at = Some(now());
        }
    }
}

/// A new worker must not recreate approvals for the previous worker's intents,
/// including intents whose target review had not yet produced an audit record.
pub(super) fn fence_pending_tools(run: &mut RunState) {
    if let Some(execution) = run.execution_mut() {
        for tool in &mut execution.tools {
            if tool.status == ToolExecutionStatus::Pending
                && matches!(
                    tool.approval_status,
                    ToolApprovalStatus::Pending | ToolApprovalStatus::Approved
                )
            {
                tool.approval_status = ToolApprovalStatus::Denied;
                tool.status = ToolExecutionStatus::Denied;
                tool.error = Some(DomainError::invariant(
                    ErrorCode::ToolApprovalRequired,
                    "worker connection ended before tool dispatch",
                ));
                tool.ended_at = Some(ait_domain::TimestampMs(now()));
            }
        }
    }
}

#[async_trait::async_trait]
impl RunApproval for LocalControlService {
    async fn decide(&self, request: ApprovalRequest) -> Result<ApprovalDecision, DomainError> {
        if request.run_id != request.execution.run_id {
            return Ok(ApprovalDecision::Denied);
        }
        self.request_api_tool_approval(request.execution, None, i64::MAX, CancellationToken::new())
            .await
            .map_err(api_domain_error)
    }
    async fn consume(&self, grant: &ToolGrant) -> Result<bool, DomainError> {
        self.consume_api_tool_grant(grant, None, &CancellationToken::new())
            .await
            .map_err(api_domain_error)
    }
}

impl LocalControlService {
    async fn review_tool(
        &self,
        run: &RunState,
        tool: &ToolExecution,
    ) -> Result<ait_domain::ToolApprovalTarget, ApiError> {
        let state = self.read_run_records(&run.id).await?.original;
        let root = run_workdir(&state, run)?;
        let factory = self.api_tools.clone().ok_or_else(invalid)?;
        let profile = run.permission_profile;
        let tool = tool.clone();
        let target = tokio::task::spawn_blocking(move || factory.review(&root, profile, &tool))
            .await
            .map_err(|_| invalid())?
            .map_err(|_| invalid())?
            .ok_or_else(invalid)?;
        if target.current != profile.sandbox
            || target.requested <= profile.sandbox
            || target.requested > self.permission_limits.max_sandbox
        {
            return Err(invalid());
        }
        // A masked command/target would be misleading authority. Reject it instead.
        ait_contracts::sensitive::validate_tool_argument_value(
            &serde_json::to_value(&target).map_err(|_| invalid())?,
        )
        .map_err(|_| invalid())?;
        Ok(target)
    }

    pub(super) fn interrupt_api_tool_approvals(&self, run_id: &str, epoch: u64) {
        if let Ok(waiters) = self.tool_approval_waiters.lock() {
            for waiter in waiters
                .values()
                .filter(|w| w.run_id == run_id && w.epoch == epoch)
            {
                waiter.connection.cancel();
                waiter.sender.send_replace(Some(Status::Expired));
            }
        }
    }

    pub(super) async fn request_api_tool_approval(
        &self,
        tool: ToolExecution,
        lease: Option<&WorkerLease>,
        worker_deadline: i64,
        connection: CancellationToken,
    ) -> Result<ApprovalDecision, ApiError> {
        let arguments_digest = digest(&tool)?;
        let id = uuid::Uuid::new_v4().to_string();
        let (sender, receiver) = tokio::sync::watch::channel(None);
        let mut guard = None;
        let mut saved = None;
        for _ in 0..8 {
            let loaded = self.read_run_records(tool.run_id.as_str()).await?;
            let mut state = loaded.original.clone();
            let run = state
                .runs
                .iter_mut()
                .find(|r| r.id == tool.run_id.as_str())
                .ok_or_else(invalid)?;
            if connection.is_cancelled()
                || !active(run, lease)
                || tool.approval_status != ToolApprovalStatus::Pending
                || tool.status != ToolExecutionStatus::Pending
                || !run.execution().is_some_and(|e| e.tools.contains(&tool))
            {
                return Ok(ApprovalDecision::Denied);
            }
            // A previous process/request cannot re-use either a pending waiter or an approval.
            if run
                .tool_approvals
                .iter()
                .any(|a| a.grant.execution_id == tool.id.as_str())
            {
                expire(run, Status::Expired);
                let event = pending("run.tool_approval_expired", Some(run.id.clone()), run);
                match self.persist_records(&loaded, &state, vec![event]).await {
                    Ok(()) => return Ok(ApprovalDecision::Denied),
                    Err(ControlStoreError::Conflict) => continue,
                    Err(e) => return Err(store_error(e)),
                }
            }
            let Ok(target) = self.review_tool(run, &tool).await else {
                return Ok(ApprovalDecision::Denied);
            };
            let expires_at = self.tool_approval_deadline(run, worker_deadline)?;
            if connection.is_cancelled() || expires_at <= now() {
                return Ok(ApprovalDecision::Denied);
            }
            if guard.is_none() {
                guard = Some(self.register_tool_waiter(run, &id, &sender, &connection)?);
            }
            let record = ToolApprovalRecord {
                grant: ToolGrant {
                    request_id: id.clone(),
                    run_id: run.id.clone(),
                    execution_id: tool.id.as_str().into(),
                    call_id: tool.call_id.clone(),
                    arguments_digest: arguments_digest.clone(),
                    target,
                    lease_epoch: run.lease_epoch,
                    expires_at,
                },
                status: Status::Pending,
                created_at: now(),
                decided_at: None,
                consumed_at: None,
            };
            run.tool_approvals.push(record.clone());
            let event = pending("run.tool_approval_requested", Some(run.id.clone()), run);
            if connection.is_cancelled() {
                return Ok(ApprovalDecision::Denied);
            }
            match self.persist_records(&loaded, &state, vec![event]).await {
                Ok(()) => {
                    saved = Some(record);
                    break;
                }
                Err(ControlStoreError::Conflict) => {}
                Err(e) => return Err(store_error(e)),
            }
        }
        let record = saved.ok_or_else(invalid)?;
        self.wait_for_tool_approval(record, lease, receiver, &connection)
            .await
    }

    fn register_tool_waiter(
        &self,
        run: &RunState,
        id: &str,
        sender: &tokio::sync::watch::Sender<Option<Status>>,
        connection: &CancellationToken,
    ) -> Result<WaitGuard, ApiError> {
        self.tool_approval_waiters
            .lock()
            .map_err(|_| invalid())?
            .insert(
                id.into(),
                ToolApprovalWaiter {
                    run_id: run.id.clone(),
                    epoch: run.lease_epoch,
                    connection: connection.clone(),
                    sender: sender.clone(),
                },
            );
        Ok(WaitGuard {
            registry: self.tool_approval_waiters.clone(),
            id: id.into(),
        })
    }

    fn tool_approval_deadline(
        &self,
        run: &RunState,
        worker_deadline: i64,
    ) -> Result<i64, ApiError> {
        let execution = run.execution().ok_or_else(invalid)?;
        let runtime_deadline = execution
            .run
            .started_at
            .zip(execution.run.budget.max_runtime)
            .map_or(i64::MAX, |(start, budget)| {
                start
                    .0
                    .saturating_add(i64::try_from(budget.0).unwrap_or(i64::MAX))
            });
        // Leave time for denied ToolResult persistence and the provider's final turn.
        let expires_at = now()
            .saturating_add(
                i64::try_from(self.tool_approval_timeout.as_millis()).unwrap_or(120_000),
            )
            .min(runtime_deadline.saturating_sub(2_000))
            .min(worker_deadline.saturating_sub(2_000));
        Ok(expires_at)
    }

    async fn wait_for_tool_approval(
        &self,
        record: ToolApprovalRecord,
        lease: Option<&WorkerLease>,
        mut receiver: tokio::sync::watch::Receiver<Option<Status>>,
        connection: &CancellationToken,
    ) -> Result<ApprovalDecision, ApiError> {
        let cancellation = self
            .cancellations
            .lock()
            .ok()
            .and_then(|c| c.get(record.grant.run_id.as_str()).cloned())
            .unwrap_or_default();
        let signal = tokio::select! {
            biased;
            () = connection.cancelled() => Status::Expired,
            () = cancellation.cancelled() => Status::Cancelled,
            () = tokio::time::sleep(Duration::from_millis(
                u64::try_from(record.grant.expires_at.saturating_sub(now())).unwrap_or(0)
            )) => Status::Expired,
            changed = receiver.changed() => {
                if changed.is_err() {
                    Status::Expired
                } else {
                    receiver.borrow_and_update().unwrap_or(Status::Expired)
                }
            },
        };
        if matches!(signal, Status::Expired | Status::Cancelled) {
            self.invalidate_api_tool_grant(&record.grant, signal)
                .await?;
        }
        let current = self
            .read_run_records(record.grant.run_id.as_str())
            .await?
            .original;
        let run = current
            .runs
            .iter()
            .find(|r| r.id == record.grant.run_id.as_str())
            .ok_or_else(invalid)?;
        if matches!(
            run.status(),
            LifecycleStatus::Cancelling | LifecycleStatus::Cancelled
        ) {
            return Ok(ApprovalDecision::Cancelled);
        }
        if !connection.is_cancelled()
            && active(run, lease)
            && now() < record.grant.expires_at
            && run
                .tool_approvals
                .iter()
                .any(|a| a.grant == record.grant && a.status == Status::Approved)
        {
            Ok(ApprovalDecision::Granted(Box::new(record.grant)))
        } else {
            Ok(ApprovalDecision::Denied)
        }
    }

    async fn invalidate_api_tool_grant(
        &self,
        grant: &ToolGrant,
        status: Status,
    ) -> Result<(), ApiError> {
        for _ in 0..8 {
            let loaded = self.read_run_records(&grant.run_id).await?;
            let mut state = loaded.original.clone();
            let run = state
                .runs
                .iter_mut()
                .find(|r| r.id == grant.run_id)
                .ok_or_else(invalid)?;
            let Some(record) = run.tool_approvals.iter_mut().find(|a| a.grant == *grant) else {
                return Ok(());
            };
            if !matches!(record.status, Status::Pending | Status::Approved) {
                return Ok(());
            }
            record.status = status;
            record.decided_at = Some(now());
            let event = pending("run.tool_approval_expired", Some(run.id.clone()), run);
            match self.persist_records(&loaded, &state, vec![event]).await {
                Ok(()) => return Ok(()),
                Err(ControlStoreError::Conflict) => {}
                Err(e) => return Err(store_error(e)),
            }
        }
        Err(invalid())
    }

    pub(super) async fn consume_api_tool_grant(
        &self,
        grant: &ToolGrant,
        lease: Option<&WorkerLease>,
        connection: &CancellationToken,
    ) -> Result<bool, ApiError> {
        for _ in 0..8 {
            let loaded = self.read_run_records(&grant.run_id).await?;
            let mut state = loaded.original.clone();
            let run = state
                .runs
                .iter_mut()
                .find(|r| r.id == grant.run_id)
                .ok_or_else(invalid)?;
            if connection.is_cancelled()
                || !active(run, lease)
                || run.lease_epoch != grant.lease_epoch
            {
                return Ok(false);
            }
            let Some(index) = run
                .tool_approvals
                .iter()
                .position(|a| a.grant == *grant && a.status == Status::Approved)
            else {
                return Ok(false);
            };
            let valid = now() < grant.expires_at;
            let tool = tool_for(run, grant)
                .filter(|t| {
                    t.status == ToolExecutionStatus::Running
                        && t.approval_status == ToolApprovalStatus::Approved
                })
                .cloned();
            let valid = if let Some(tool) = tool {
                valid
                    && digest(&tool)? == grant.arguments_digest
                    && self
                        .review_tool(run, &tool)
                        .await
                        .is_ok_and(|t| t == grant.target)
            } else {
                false
            };
            let valid = valid && !connection.is_cancelled() && now() < grant.expires_at;
            run.tool_approvals[index].status = if valid {
                Status::Consumed
            } else {
                Status::Expired
            };
            run.tool_approvals[index].consumed_at = valid.then(now);
            let event = pending("run.tool_approval_consumed", Some(run.id.clone()), run);
            match self.persist_records(&loaded, &state, vec![event]).await {
                Ok(()) => {
                    return Ok(valid && !connection.is_cancelled() && now() < grant.expires_at);
                }
                Err(ControlStoreError::Conflict) => {}
                Err(e) => return Err(store_error(e)),
            }
        }
        Err(invalid())
    }

    /// Resolve a persisted API tool request. Approve grants this operation only.
    /// # Errors
    /// Rejects stale, duplicate, unsupported, expired, or changed authorization.
    pub async fn resolve_tool_approval(
        &self,
        run_id: &str,
        approval_id: &str,
        action: ToolApprovalAction,
    ) -> Result<RunView, ApiError> {
        for _ in 0..8 {
            let loaded = self.read_run_records(run_id).await?;
            let mut state = loaded.original.clone();
            let run = state
                .runs
                .iter_mut()
                .find(|r| r.id == run_id)
                .ok_or_else(invalid)?;
            let index = run
                .tool_approvals
                .iter()
                .position(|a| a.grant.request_id == approval_id)
                .ok_or_else(invalid)?;
            let record = run.tool_approvals[index].clone();
            let connection = self
                .tool_approval_waiters
                .lock()
                .map_err(|_| invalid())?
                .get(approval_id)
                .filter(|w| w.run_id == run.id && w.epoch == run.lease_epoch)
                .map(|w| w.connection.clone());
            if !active(run, None) || record.status != Status::Pending {
                return Err(invalid());
            }
            if connection
                .as_ref()
                .is_none_or(CancellationToken::is_cancelled)
                || now() >= record.grant.expires_at
                || record.grant.lease_epoch != run.lease_epoch
            {
                self.invalidate_api_tool_grant(&record.grant, Status::Expired)
                    .await?;
                return Err(invalid());
            }
            let connection = connection.ok_or_else(invalid)?;
            if action == ToolApprovalAction::Cancel {
                let response = self
                    .execute(Command::CancelRun {
                        run_id: run_id.into(),
                    })
                    .await;
                return match response.result {
                    Some(CommandResult::Run(run)) => Ok(run),
                    _ => Err(response.error.unwrap_or_else(invalid)),
                };
            }
            if action == ToolApprovalAction::Approve {
                let tool = tool_for(run, &record.grant)
                    .filter(|t| {
                        t.status == ToolExecutionStatus::Pending
                            && t.approval_status == ToolApprovalStatus::Pending
                    })
                    .ok_or_else(invalid)?;
                if digest(tool)? != record.grant.arguments_digest
                    || !self
                        .review_tool(run, tool)
                        .await
                        .is_ok_and(|target| target == record.grant.target)
                {
                    self.invalidate_api_tool_grant(&record.grant, Status::Expired)
                        .await?;
                    self.notify_api_tool_waiter(approval_id, Status::Expired);
                    return Err(invalid());
                }
            }
            if connection.is_cancelled() || now() >= record.grant.expires_at {
                self.invalidate_api_tool_grant(&record.grant, Status::Expired)
                    .await?;
                self.notify_api_tool_waiter(approval_id, Status::Expired);
                return Err(invalid());
            }
            let status = if action == ToolApprovalAction::Approve {
                Status::Approved
            } else {
                Status::Denied
            };
            run.tool_approvals[index].status = status;
            run.tool_approvals[index].decided_at = Some(now());
            let view = run.view();
            let event = pending("run.tool_approval_resolved", Some(run.id.clone()), run);
            match self.persist_records(&loaded, &state, vec![event]).await {
                Ok(()) => {
                    if connection.is_cancelled() || now() >= record.grant.expires_at {
                        self.invalidate_api_tool_grant(&record.grant, Status::Expired)
                            .await?;
                        self.notify_api_tool_waiter(approval_id, Status::Expired);
                        return Err(invalid());
                    }
                    self.notify_api_tool_waiter(approval_id, status);
                    return Ok(view);
                }
                Err(ControlStoreError::Conflict) => {}
                Err(e) => return Err(store_error(e)),
            }
        }
        Err(invalid())
    }
    fn notify_api_tool_waiter(&self, id: &str, status: Status) {
        if let Ok(waiters) = self.tool_approval_waiters.lock()
            && let Some(waiter) = waiters.get(id)
        {
            waiter.sender.send_replace(Some(status));
        }
    }
}
