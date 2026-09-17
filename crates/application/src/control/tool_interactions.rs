//! Durable member responses for API question and plan-review tools.

use super::errors::{api_domain_error, error, store_error};
use super::events::{now, pending};
use super::{LocalControlService, model::ToolInteractionState};
use ait_contracts::{ApiError, Command, CommandResult, RunView, ToolInteractionAction};
use ait_domain::{DomainError, ErrorCode, LifecycleStatus, ToolExecution, ToolExecutionStatus};
use ait_ports::{
    ControlStoreError, RunToolInteraction, ToolInvocation, ToolOutcome, ToolRecovery, WorkerLease,
};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
enum Signal {
    Answered(Value),
    Cancelled,
    Expired,
}

pub(super) struct ToolInteractionWaiter {
    run_id: String,
    epoch: u64,
    connection: CancellationToken,
    sender: tokio::sync::watch::Sender<Option<Signal>>,
}

struct WaitGuard {
    registry: Arc<Mutex<HashMap<String, ToolInteractionWaiter>>>,
    id: String,
    epoch: u64,
}

impl Drop for WaitGuard {
    fn drop(&mut self) {
        if let Ok(mut waiters) = self.registry.lock()
            && waiters
                .get(&self.id)
                .is_some_and(|waiter| waiter.epoch == self.epoch)
        {
            waiters.remove(&self.id);
        }
    }
}

fn invalid() -> ApiError {
    error(
        ErrorCode::ToolExecutionFailed,
        "tool interaction is invalid, expired, or no longer pending",
        false,
    )
}

fn active(run: &super::model::RunState, lease: Option<&WorkerLease>) -> bool {
    !run.status().is_terminal()
        && run.status() != LifecycleStatus::Cancelling
        && lease.is_none_or(|lease| {
            run.id == lease.run_id.as_str()
                && run.lease_epoch == lease.epoch
                && run
                    .execution()
                    .and_then(|execution| execution.worker_instance_id.as_deref())
                    == Some(lease.instance_id.as_str())
        })
}

fn matching_tool(run: &super::model::RunState, request: &ToolInvocation) -> bool {
    run.execution().is_some_and(|execution| {
        execution.tools.iter().any(|tool| {
            tool.id == request.execution_id
                && tool.run_id == request.run_id
                && tool.call_id == request.call_id
                && tool.tool_name == request.tool_name
                && tool.arguments == request.arguments
                && tool.status == ToolExecutionStatus::Running
        })
    })
}

fn validate_answers(request: &Value, response: &Value) -> Result<(), ApiError> {
    let questions = request["questions"].as_array().ok_or_else(invalid)?;
    let answers = response.as_object().ok_or_else(invalid)?;
    if questions.is_empty() || questions.len() > 10 || answers.len() != questions.len() {
        return Err(invalid());
    }
    for question in questions {
        let id = question["id"].as_str().ok_or_else(invalid)?;
        let answer = answers.get(id).ok_or_else(invalid)?;
        let options = question.get("options").and_then(Value::as_array);
        let multi = question["multi_select"].as_bool().unwrap_or(false);
        let allowed = |value: &str| {
            options.is_none_or(|options| {
                options
                    .iter()
                    .any(|option| option["label"].as_str() == Some(value))
            })
        };
        let valid = if multi {
            answer.as_array().is_some_and(|values| {
                let mut seen = std::collections::HashSet::new();
                !values.is_empty()
                    && values.len() <= 20
                    && values.iter().all(|value| {
                        value.as_str().is_some_and(|value| {
                            !value.trim().is_empty()
                                && value.len() <= 1_024
                                && allowed(value)
                                && seen.insert(value)
                        })
                    })
            })
        } else {
            answer.as_str().is_some_and(|value| {
                !value.trim().is_empty() && value.len() <= 4_096 && allowed(value)
            })
        };
        if !valid {
            return Err(invalid());
        }
    }
    Ok(())
}

impl LocalControlService {
    fn register_tool_interaction_waiter(
        &self,
        id: &str,
        run_id: &str,
        epoch: u64,
        connection: &CancellationToken,
        sender: &tokio::sync::watch::Sender<Option<Signal>>,
    ) -> Result<(), ApiError> {
        self.tool_interaction_waiters
            .lock()
            .map_err(|_| invalid())?
            .insert(
                id.to_owned(),
                ToolInteractionWaiter {
                    run_id: run_id.to_owned(),
                    epoch,
                    connection: connection.clone(),
                    sender: sender.clone(),
                },
            );
        Ok(())
    }

    fn track_tool_interaction_waiter(&self, guard: &mut Option<WaitGuard>, id: &str, epoch: u64) {
        match guard {
            Some(guard) => guard.epoch = epoch,
            None => {
                *guard = Some(WaitGuard {
                    registry: self.tool_interaction_waiters.clone(),
                    id: id.to_owned(),
                    epoch,
                });
            }
        }
    }

    fn interaction_deadline(
        &self,
        run: &super::model::RunState,
        worker_deadline: i64,
    ) -> Result<i64, ApiError> {
        let runtime_deadline = run
            .execution()
            .ok_or_else(invalid)?
            .run
            .started_at
            .zip(
                run.execution()
                    .and_then(|execution| execution.run.budget.max_runtime),
            )
            .map_or(i64::MAX, |(start, budget)| {
                start
                    .0
                    .saturating_add(i64::try_from(budget.0).unwrap_or(i64::MAX))
            });
        Ok(now()
            .saturating_add(
                i64::try_from(self.tool_approval_timeout.as_millis()).unwrap_or(120_000),
            )
            .min(runtime_deadline.saturating_sub(2_000))
            .min(worker_deadline.saturating_sub(2_000)))
    }

    async fn await_tool_interaction(
        &self,
        request: &ToolInvocation,
        id: &str,
        expires_at: i64,
        connection: CancellationToken,
        mut receiver: tokio::sync::watch::Receiver<Option<Signal>>,
    ) -> Result<ToolOutcome, ApiError> {
        let cancellation = self
            .cancellations
            .lock()
            .ok()
            .and_then(|values| values.get(request.run_id.as_str()).cloned())
            .unwrap_or_default();
        let signal = tokio::select! {
            biased;
            () = cancellation.cancelled() => Signal::Cancelled,
            () = connection.cancelled() => Signal::Expired,
            () = request.cancellation.cancelled() => Signal::Cancelled,
            () = tokio::time::sleep(std::time::Duration::from_millis(
                u64::try_from(expires_at.saturating_sub(now())).unwrap_or(0),
            )) => Signal::Expired,
            changed = receiver.changed() => {
                if changed.is_err() {
                    Signal::Expired
                } else {
                    receiver.borrow_and_update().clone().unwrap_or(Signal::Expired)
                }
            }
        };
        match signal {
            Signal::Answered(output) => Ok(ToolOutcome {
                output,
                usage: ait_domain::RunUsage::default(),
            }),
            Signal::Cancelled => {
                self.finish_tool_interaction(request.run_id.as_str(), id, "cancelled")
                    .await?;
                Err(error(
                    ErrorCode::RunCancelled,
                    "tool interaction cancelled",
                    false,
                ))
            }
            Signal::Expired => {
                self.finish_tool_interaction(request.run_id.as_str(), id, "expired")
                    .await?;
                Err(invalid())
            }
        }
    }

    pub(super) async fn request_api_tool_interaction(
        &self,
        request: ToolInvocation,
        lease: Option<&WorkerLease>,
        worker_deadline: i64,
        connection: CancellationToken,
    ) -> Result<ToolOutcome, ApiError> {
        let id = request.execution_id.as_str().to_owned();
        let (sender, receiver) = tokio::sync::watch::channel(None);
        let mut guard: Option<WaitGuard> = None;
        let mut expires_at = None;
        let mut persisted = false;
        for _ in 0..8 {
            let loaded = self.read_run_records(request.run_id.as_str()).await?;
            let mut state = loaded.original.clone();
            let run = state
                .runs
                .iter_mut()
                .find(|run| run.id == request.run_id.as_str())
                .ok_or_else(invalid)?;
            if connection.is_cancelled() || !active(run, lease) || !matching_tool(run, &request) {
                return Err(invalid());
            }
            if let Some(existing) = run
                .tool_interactions
                .iter()
                .find(|interaction| interaction.id == id)
            {
                if existing.tool_name != request.tool_name || existing.request != request.arguments
                {
                    return Err(invalid());
                }
                if let Some(response) = &existing.response {
                    return Ok(ToolOutcome {
                        output: response.clone(),
                        usage: ait_domain::RunUsage::default(),
                    });
                }
                if existing.status == "pending"
                    && existing.lease_epoch == run.lease_epoch
                    && existing.expires_at > now()
                {
                    expires_at = Some(existing.expires_at);
                } else {
                    let index = run
                        .tool_interactions
                        .iter()
                        .position(|interaction| interaction.id == id)
                        .ok_or_else(invalid)?;
                    run.tool_interactions[index].status = "pending".into();
                    run.tool_interactions[index].lease_epoch = run.lease_epoch;
                    run.tool_interactions[index].expires_at =
                        self.interaction_deadline(run, worker_deadline)?;
                    run.tool_interactions[index].decided_at = None;
                    expires_at = Some(run.tool_interactions[index].expires_at);
                }
            } else {
                let deadline = self.interaction_deadline(run, worker_deadline)?;
                if deadline <= now() {
                    return Err(invalid());
                }
                run.tool_interactions.push(ToolInteractionState {
                    id: id.clone(),
                    run_id: run.id.clone(),
                    tool_name: request.tool_name.clone(),
                    request: request.arguments.clone(),
                    response: None,
                    status: "pending".into(),
                    lease_epoch: run.lease_epoch,
                    expires_at: deadline,
                    created_at: now(),
                    decided_at: None,
                });
                expires_at = Some(deadline);
            }
            self.register_tool_interaction_waiter(
                &id,
                &run.id,
                run.lease_epoch,
                &connection,
                &sender,
            )?;
            self.track_tool_interaction_waiter(&mut guard, &id, run.lease_epoch);
            let event = pending("run.tool_interaction_requested", Some(run.id.clone()), run);
            match self.persist_records(&loaded, &state, vec![event]).await {
                Ok(()) => {
                    persisted = true;
                    break;
                }
                Err(ControlStoreError::Conflict) => {}
                Err(failure) => return Err(store_error(failure)),
            }
        }
        if !persisted {
            return Err(invalid());
        }
        let expires_at = expires_at.ok_or_else(invalid)?;
        let result = self
            .await_tool_interaction(&request, &id, expires_at, connection, receiver)
            .await;
        drop(guard);
        result
    }

    async fn finish_tool_interaction(
        &self,
        run_id: &str,
        id: &str,
        status: &'static str,
    ) -> Result<(), ApiError> {
        for _ in 0..8 {
            let loaded = self.read_run_records(run_id).await?;
            let mut state = loaded.original.clone();
            let run = state
                .runs
                .iter_mut()
                .find(|run| run.id == run_id)
                .ok_or_else(invalid)?;
            let Some(interaction) = run
                .tool_interactions
                .iter_mut()
                .find(|interaction| interaction.id == id && interaction.status == "pending")
            else {
                return Ok(());
            };
            interaction.status = status.into();
            interaction.decided_at = Some(now());
            let event = pending(
                if status == "cancelled" {
                    "run.tool_interaction_cancelled"
                } else {
                    "run.tool_interaction_expired"
                },
                Some(run.id.clone()),
                run,
            );
            match self.persist_records(&loaded, &state, vec![event]).await {
                Ok(()) => return Ok(()),
                Err(ControlStoreError::Conflict) => {}
                Err(failure) => return Err(store_error(failure)),
            }
        }
        Err(invalid())
    }

    pub(super) async fn recover_api_tool_interaction(
        &self,
        execution: &ToolExecution,
    ) -> Result<ToolRecovery, ApiError> {
        let state = self
            .read_run_records(execution.run_id.as_str())
            .await?
            .original;
        let run = state
            .runs
            .iter()
            .find(|run| run.id == execution.run_id.as_str())
            .ok_or_else(invalid)?;
        let Some(interaction) = run
            .tool_interactions
            .iter()
            .find(|interaction| interaction.id == execution.id.as_str())
        else {
            return Ok(ToolRecovery::RetrySafe);
        };
        if interaction.tool_name != execution.tool_name
            || interaction.request != execution.arguments
        {
            return Ok(ToolRecovery::Unknown);
        }
        if let Some(output) = &interaction.response {
            return Ok(ToolRecovery::Completed(ToolOutcome {
                output: output.clone(),
                usage: ait_domain::RunUsage::default(),
            }));
        }
        Ok(ToolRecovery::RetrySafe)
    }

    /// Resolve a pending API question or plan review.
    ///
    /// # Errors
    ///
    /// Returns an error for stale, expired, mismatched, or invalid responses,
    /// or when the durable state transition cannot be persisted.
    pub async fn resolve_tool_interaction(
        &self,
        run_id: &str,
        interaction_id: &str,
        action: ToolInteractionAction,
        response: Option<Value>,
    ) -> Result<RunView, ApiError> {
        if action == ToolInteractionAction::Cancel {
            if response.is_some() {
                return Err(invalid());
            }
            let state = self.read_run_records(run_id).await?.original;
            let run = state
                .runs
                .iter()
                .find(|run| run.id == run_id)
                .ok_or_else(invalid)?;
            if !active(run, None)
                || !run.tool_interactions.iter().any(|interaction| {
                    interaction.id == interaction_id
                        && interaction.status == "pending"
                        && interaction.expires_at > now()
                })
            {
                return Err(invalid());
            }
            let result = self
                .execute(Command::CancelRun {
                    run_id: run_id.into(),
                })
                .await;
            return match result.result {
                Some(CommandResult::Run(run)) => Ok(run),
                _ => Err(result.error.unwrap_or_else(invalid)),
            };
        }
        for _ in 0..8 {
            let loaded = self.read_run_records(run_id).await?;
            let mut state = loaded.original.clone();
            let run = state
                .runs
                .iter_mut()
                .find(|run| run.id == run_id)
                .ok_or_else(invalid)?;
            let index = run
                .tool_interactions
                .iter()
                .position(|interaction| interaction.id == interaction_id)
                .ok_or_else(invalid)?;
            let record = run.tool_interactions[index].clone();
            let connection = self
                .tool_interaction_waiters
                .lock()
                .map_err(|_| invalid())?
                .get(interaction_id)
                .filter(|waiter| waiter.run_id == run.id && waiter.epoch == run.lease_epoch)
                .map(|waiter| waiter.connection.clone())
                .ok_or_else(invalid)?;
            if !active(run, None)
                || record.status != "pending"
                || record.expires_at <= now()
                || connection.is_cancelled()
            {
                return Err(invalid());
            }
            let output = match (record.tool_name.as_str(), action, response.as_ref()) {
                ("ask_user_question", ToolInteractionAction::Submit, Some(answers)) => {
                    validate_answers(&record.request, answers)?;
                    json!({"answers": answers})
                }
                ("exit_plan_mode", ToolInteractionAction::Approve, None) => {
                    json!({"approved":true})
                }
                ("exit_plan_mode", ToolInteractionAction::Deny, None) => {
                    json!({"approved":false})
                }
                _ => return Err(invalid()),
            };
            run.tool_interactions[index].response = Some(output.clone());
            run.tool_interactions[index].status = match action {
                ToolInteractionAction::Submit => "answered",
                ToolInteractionAction::Approve => "approved",
                ToolInteractionAction::Deny => "denied",
                ToolInteractionAction::Cancel => unreachable!(),
            }
            .into();
            run.tool_interactions[index].decided_at = Some(now());
            let view = run.view();
            let event = pending("run.tool_interaction_resolved", Some(run.id.clone()), run);
            match self.persist_records(&loaded, &state, vec![event]).await {
                Ok(()) => {
                    if let Ok(waiters) = self.tool_interaction_waiters.lock()
                        && let Some(waiter) = waiters.get(interaction_id)
                    {
                        waiter.sender.send_replace(Some(Signal::Answered(output)));
                    }
                    return Ok(view);
                }
                Err(ControlStoreError::Conflict) => {}
                Err(failure) => return Err(store_error(failure)),
            }
        }
        Err(invalid())
    }

    pub(super) fn interrupt_tool_interactions(&self, run_id: &str, epoch: u64) {
        if let Ok(waiters) = self.tool_interaction_waiters.lock() {
            for waiter in waiters
                .values()
                .filter(|waiter| waiter.run_id == run_id && waiter.epoch == epoch)
            {
                waiter.connection.cancel();
                waiter.sender.send_replace(Some(Signal::Expired));
            }
        }
    }
}

#[async_trait::async_trait]
impl RunToolInteraction for LocalControlService {
    async fn request(&self, request: ToolInvocation) -> Result<ToolOutcome, DomainError> {
        self.request_api_tool_interaction(request, None, i64::MAX, CancellationToken::new())
            .await
            .map_err(api_domain_error)
    }

    async fn reconcile(&self, execution: &ToolExecution) -> Result<ToolRecovery, DomainError> {
        self.recover_api_tool_interaction(execution)
            .await
            .map_err(api_domain_error)
    }
}
