//! `RunStore` client/server adapters over the private worker protocol.
use crate::mapping::Wire;
use ait_contracts::worker::{Lease, ProtocolError, StoreRequest, StoreResponse};
use ait_domain::{Message, MessageId, ProjectedMessage, Run, RunAttempt, RunId, ToolExecution};
use ait_ports::{CompletionResult, RunMutation, RunStore, RunStoreError, WorkerLease};
use async_trait::async_trait;
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot};

/// Bounded local request sent to the worker's sole protocol driver.
pub struct Call {
    /// Versioned request.
    pub request: StoreRequest,
    /// Response delivered after the matching commit ACK.
    pub reply: oneshot::Sender<Result<StoreResponse, ProtocolError>>,
}
/// Worker-side durable state port; owns no filesystem/database access.
#[derive(Clone)]
pub struct RemoteStore {
    sender: mpsc::Sender<Call>,
}
impl RemoteStore {
    /// Creates a bounded request queue for the worker protocol driver.
    #[must_use]
    pub fn channel() -> (Self, mpsc::Receiver<Call>) {
        let (sender, receiver) = mpsc::channel(16);
        (Self { sender }, receiver)
    }
    /// Executes a versioned port request, waiting for its correlated ACK.
    /// # Errors
    /// Returns a redacted channel error on failure.
    pub async fn call(&self, request: StoreRequest) -> Result<StoreResponse, RunStoreError> {
        let (reply, response) = oneshot::channel();
        self.sender
            .send(Call { request, reply })
            .await
            .map_err(|_| failure())?;
        response
            .await
            .map_err(|_| failure())?
            .map_err(|_| failure())
    }
    async fn run(&self, request: StoreRequest) -> Result<Run, RunStoreError> {
        match self.call(request).await? {
            StoreResponse::Run { run } => Run::from_wire(*run).map_err(|_| failure()),
            _ => Err(failure()),
        }
    }
}
fn failure() -> RunStoreError {
    RunStoreError::Other("worker state channel unavailable or commit rejected".into())
}

#[async_trait]
impl ait_ports::RunApproval for RemoteStore {
    async fn consume(
        &self,
        grant: &ait_domain::ToolGrant,
    ) -> Result<bool, ait_domain::DomainError> {
        match self
            .call(StoreRequest::ConsumeToolGrant {
                grant: Box::new(grant.clone()),
            })
            .await
        {
            Ok(StoreResponse::Approval { decision }) => Ok(decision == "consumed"),
            _ => Ok(false),
        }
    }
    async fn decide(
        &self,
        request: ait_ports::ApprovalRequest,
    ) -> Result<ait_ports::ApprovalDecision, ait_domain::DomainError> {
        let failed = || {
            ait_domain::DomainError::invariant(
                ait_domain::ErrorCode::ToolApprovalRequired,
                "daemon approval port unavailable",
            )
        };
        if request.execution.run_id != request.run_id {
            return Err(failed());
        }
        match self
            .call(StoreRequest::Approval {
                execution: Box::new(request.execution.to_wire()),
            })
            .await
            .map_err(|_| failed())?
        {
            StoreResponse::ToolGrant { grant } => Ok(ait_ports::ApprovalDecision::Granted(grant)),
            StoreResponse::Approval { decision } if decision == "cancelled" => {
                Ok(ait_ports::ApprovalDecision::Cancelled)
            }
            StoreResponse::Approval { decision } if decision == "denied" => {
                Ok(ait_ports::ApprovalDecision::Denied)
            }
            _ => Err(failed()),
        }
    }
}

#[async_trait]
impl ait_ports::RunToolInteraction for RemoteStore {
    async fn request(
        &self,
        request: ait_ports::ToolInvocation,
    ) -> Result<ait_ports::ToolOutcome, ait_domain::DomainError> {
        let failed = || {
            ait_domain::DomainError::invariant(
                ait_domain::ErrorCode::ToolExecutionFailed,
                "daemon tool interaction channel unavailable",
            )
        };
        match self
            .call(StoreRequest::ToolInteraction {
                request: Box::new(ait_contracts::worker::ToolInteractionRequest {
                    run_id: request.run_id.as_str().into(),
                    call_id: request.call_id,
                    execution_id: request.execution_id.as_str().into(),
                    tool_name: request.tool_name,
                    arguments: request.arguments,
                }),
            })
            .await
            .map_err(|_| failed())?
        {
            StoreResponse::ToolInteraction { output } => Ok(ait_ports::ToolOutcome {
                output,
                usage: ait_domain::RunUsage::default(),
            }),
            _ => Err(failed()),
        }
    }

    async fn reconcile(
        &self,
        execution: &ToolExecution,
    ) -> Result<ait_ports::ToolRecovery, ait_domain::DomainError> {
        let failed = || {
            ait_domain::DomainError::invariant(
                ait_domain::ErrorCode::RunRecoveryFailed,
                "daemon tool interaction recovery unavailable",
            )
        };
        match self
            .call(StoreRequest::ToolInteractionRecovery {
                execution: Box::new(execution.to_wire()),
            })
            .await
            .map_err(|_| failed())?
        {
            StoreResponse::ToolRecovery { recovery, output } if recovery == "completed" => {
                Ok(ait_ports::ToolRecovery::Completed(ait_ports::ToolOutcome {
                    output: output.ok_or_else(failed)?,
                    usage: ait_domain::RunUsage::default(),
                }))
            }
            StoreResponse::ToolRecovery { recovery, .. } if recovery == "retry_safe" => {
                Ok(ait_ports::ToolRecovery::RetrySafe)
            }
            StoreResponse::ToolRecovery { recovery, .. } if recovery == "unknown" => {
                Ok(ait_ports::ToolRecovery::Unknown)
            }
            _ => Err(failed()),
        }
    }
}

#[async_trait]
impl RunStore for RemoteStore {
    async fn load_run(&self, _: &RunId) -> Result<Run, RunStoreError> {
        self.run(StoreRequest::LoadRun).await
    }
    async fn load_message_path(
        &self,
        head: &MessageId,
    ) -> Result<Vec<ProjectedMessage>, RunStoreError> {
        let mut all = Vec::new();
        let mut offset = 0;
        loop {
            let StoreResponse::Messages { entries, next } = self
                .call(StoreRequest::MessagePath {
                    head: head.to_wire(),
                    offset,
                })
                .await?
            else {
                return Err(failure());
            };
            all.extend(Vec::<ProjectedMessage>::from_wire(entries).map_err(|_| failure())?);
            if all.len() > 8192 {
                return Err(failure());
            }
            match next {
                Some(next) if next > offset => offset = next,
                None => return Ok(all),
                _ => return Err(failure()),
            }
        }
    }
    async fn load_attempts(&self, _: &RunId) -> Result<Vec<RunAttempt>, RunStoreError> {
        let mut all = Vec::new();
        let mut offset = 0;
        loop {
            let StoreResponse::Attempts { entries, next } =
                self.call(StoreRequest::Attempts { offset }).await?
            else {
                return Err(failure());
            };
            all.extend(Vec::<RunAttempt>::from_wire(entries).map_err(|_| failure())?);
            if all.len() > 8192 {
                return Err(failure());
            }
            match next {
                Some(next) if next > offset => offset = next,
                None => return Ok(all),
                _ => return Err(failure()),
            }
        }
    }
    async fn load_tool_executions(
        &self,
        _: &RunId,
        assistant: &MessageId,
    ) -> Result<Vec<ToolExecution>, RunStoreError> {
        let mut all = Vec::new();
        let mut offset = 0;
        loop {
            let StoreResponse::Tools { entries, next } = self
                .call(StoreRequest::Tools {
                    assistant: assistant.to_wire(),
                    offset,
                })
                .await?
            else {
                return Err(failure());
            };
            all.extend(Vec::<ToolExecution>::from_wire(entries).map_err(|_| failure())?);
            if all.len() > 8192 {
                return Err(failure());
            }
            match next {
                Some(next) if next > offset => offset = next,
                None => return Ok(all),
                _ => return Err(failure()),
            }
        }
    }
    async fn save_run(&self, run: Run) -> Result<Run, RunStoreError> {
        self.run(StoreRequest::SaveRun {
            run: Box::new(run.to_wire()),
        })
        .await
    }
    async fn save_attempt(&self, run: Run, attempt: RunAttempt) -> Result<Run, RunStoreError> {
        self.run(StoreRequest::SaveAttempt {
            run: Box::new(run.to_wire()),
            attempt: attempt.to_wire(),
        })
        .await
    }
    async fn append_message(&self, run: Run, message: Message) -> Result<Run, RunStoreError> {
        self.run(StoreRequest::AppendMessage {
            run: Box::new(run.to_wire()),
            message: Box::new(message.to_wire()),
        })
        .await
    }
    async fn save_tool_execution(
        &self,
        run: Run,
        tool: ToolExecution,
    ) -> Result<Run, RunStoreError> {
        self.run(StoreRequest::SaveTool {
            run: Box::new(run.to_wire()),
            tool: Box::new(tool.to_wire()),
        })
        .await
    }
    async fn append_tool_result(
        &self,
        run: Run,
        tool: ToolExecution,
        message: Message,
    ) -> Result<Run, RunStoreError> {
        self.run(StoreRequest::AppendToolResult {
            run: Box::new(run.to_wire()),
            tool: Box::new(tool.to_wire()),
            message: Box::new(message.to_wire()),
        })
        .await
    }
    async fn try_complete(
        &self,
        run: Run,
        queue_version: u64,
    ) -> Result<CompletionResult, RunStoreError> {
        match self
            .call(StoreRequest::Complete {
                run: Box::new(run.to_wire()),
                queue_version,
            })
            .await?
        {
            StoreResponse::Completion { run, completed } => {
                let run = Run::from_wire(*run).map_err(|_| failure())?;
                Ok(if completed {
                    CompletionResult::Completed(run)
                } else {
                    CompletionResult::QueueChanged(run)
                })
            }
            _ => Err(failure()),
        }
    }
    async fn drain_queue(&self, run: Run) -> Result<Run, RunStoreError> {
        self.run(StoreRequest::DrainQueue {
            run: Box::new(run.to_wire()),
        })
        .await
    }
}

/// Daemon-side adapter. Every mutation is fenced again inside the store transaction.
pub struct StoreServer {
    /// Existing authoritative store, not a worker-owned copy.
    pub store: Arc<dyn RunStore>,
    /// This connection's lease.
    pub lease: Lease,
}
impl StoreServer {
    /// Execute one correlated request. Caller must serialize store calls.
    ///
    /// # Errors
    /// Returns stable, redacted boundary errors.
    #[allow(
        clippy::too_many_lines,
        reason = "explicit protocol-to-port dispatch keeps all accepted operations visible"
    )]
    pub async fn handle(
        &self,
        lease: &Lease,
        operation_id: &str,
        request: StoreRequest,
    ) -> Result<StoreResponse, ProtocolError> {
        if lease.run_id != self.lease.run_id {
            return Err(ProtocolError::WrongRun);
        }
        if lease != &self.lease {
            return Err(ProtocolError::StaleWorkerLease);
        }
        validate_private_input(&request)?;
        let id = RunId::new(&lease.run_id);
        let rejected = |_| ProtocolError::InvalidTransition;
        let mutation = match request {
            StoreRequest::WorkspaceProgress { .. }
            | StoreRequest::WorkspaceCheckpoint { .. }
            | StoreRequest::WorkspaceIntegration
            | StoreRequest::WorkspaceApproval { .. }
            | StoreRequest::WorkspaceFinished { .. } => return Err(ProtocolError::InvalidFrame),
            StoreRequest::LoadRun => {
                return Ok(StoreResponse::Run {
                    run: Box::new(self.store.load_run(&id).await.map_err(rejected)?.to_wire()),
                });
            }
            StoreRequest::MessagePath { head, offset } => {
                let run = self.store.load_run(&id).await.map_err(rejected)?;
                let head = MessageId::from_wire(head)?;
                if head != run.last_message_id.unwrap_or(run.base_message_id) {
                    return Err(ProtocolError::WrongRun);
                }
                let (entries, next) = page(
                    self.store
                        .load_message_path(&head)
                        .await
                        .map_err(rejected)?
                        .to_wire(),
                    offset,
                )?;
                return Ok(StoreResponse::Messages { entries, next });
            }
            StoreRequest::Attempts { offset } => {
                let (entries, next) = page(
                    self.store
                        .load_attempts(&id)
                        .await
                        .map_err(rejected)?
                        .to_wire(),
                    offset,
                )?;
                return Ok(StoreResponse::Attempts { entries, next });
            }
            StoreRequest::Tools { assistant, offset } => {
                let (entries, next) = page(
                    self.store
                        .load_tool_executions(&id, &MessageId::from_wire(assistant)?)
                        .await
                        .map_err(rejected)?
                        .to_wire(),
                    offset,
                )?;
                return Ok(StoreResponse::Tools { entries, next });
            }
            StoreRequest::Approval { execution } => {
                if execution.run_id != lease.run_id {
                    return Err(ProtocolError::WrongRun);
                }
                let decision = self
                    .store
                    .request_tool_approval(
                        &WorkerLease {
                            run_id: id,
                            instance_id: lease.worker_instance_id.clone(),
                            epoch: lease.lease_epoch,
                        },
                        ToolExecution::from_wire(*execution)?,
                    )
                    .await
                    .map_err(|_| ProtocolError::InvalidTransition)?;
                return Ok(match decision {
                    ait_ports::ApprovalDecision::Granted(grant) => {
                        StoreResponse::ToolGrant { grant }
                    }
                    ait_ports::ApprovalDecision::Cancelled => StoreResponse::Approval {
                        decision: "cancelled".into(),
                    },
                    _ => StoreResponse::Approval {
                        decision: "denied".into(),
                    },
                });
            }
            StoreRequest::ConsumeToolGrant { grant } => {
                if grant.run_id != lease.run_id {
                    return Err(ProtocolError::WrongRun);
                }
                let consumed = self
                    .store
                    .consume_tool_grant(
                        &WorkerLease {
                            run_id: id,
                            instance_id: lease.worker_instance_id.clone(),
                            epoch: lease.lease_epoch,
                        },
                        &grant,
                    )
                    .await
                    .map_err(|_| ProtocolError::InvalidTransition)?;
                return Ok(StoreResponse::Approval {
                    decision: if consumed { "consumed" } else { "denied" }.into(),
                });
            }
            StoreRequest::ToolInteraction { request } => {
                if request.run_id != lease.run_id {
                    return Err(ProtocolError::WrongRun);
                }
                let output = self
                    .store
                    .request_tool_interaction(
                        &WorkerLease {
                            run_id: id,
                            instance_id: lease.worker_instance_id.clone(),
                            epoch: lease.lease_epoch,
                        },
                        request.call_id,
                        request.execution_id,
                        request.tool_name,
                        request.arguments,
                    )
                    .await
                    .map_err(|_| ProtocolError::InvalidTransition)?;
                return Ok(StoreResponse::ToolInteraction {
                    output: output.output,
                });
            }
            StoreRequest::ToolInteractionRecovery { execution } => {
                if execution.run_id != lease.run_id {
                    return Err(ProtocolError::WrongRun);
                }
                let recovery = self
                    .store
                    .recover_tool_interaction(&ToolExecution::from_wire(*execution)?)
                    .await
                    .map_err(|_| ProtocolError::InvalidTransition)?;
                return Ok(match recovery {
                    ait_ports::ToolRecovery::Completed(outcome) => StoreResponse::ToolRecovery {
                        recovery: "completed".into(),
                        output: Some(outcome.output),
                    },
                    ait_ports::ToolRecovery::RetrySafe => StoreResponse::ToolRecovery {
                        recovery: "retry_safe".into(),
                        output: None,
                    },
                    ait_ports::ToolRecovery::Unknown => StoreResponse::ToolRecovery {
                        recovery: "unknown".into(),
                        output: None,
                    },
                });
            }
            StoreRequest::SaveRun { run } => RunMutation::SaveRun(Run::from_wire(*run)?),
            StoreRequest::SaveAttempt { run, attempt } => {
                RunMutation::SaveAttempt(Run::from_wire(*run)?, RunAttempt::from_wire(attempt)?)
            }
            StoreRequest::AppendMessage { run, message } => {
                RunMutation::AppendMessage(Run::from_wire(*run)?, Message::from_wire(*message)?)
            }
            StoreRequest::SaveTool { run, tool } => {
                RunMutation::SaveTool(Run::from_wire(*run)?, ToolExecution::from_wire(*tool)?)
            }
            StoreRequest::AppendToolResult { run, tool, message } => RunMutation::AppendToolResult(
                Run::from_wire(*run)?,
                ToolExecution::from_wire(*tool)?,
                Message::from_wire(*message)?,
            ),
            StoreRequest::Complete { run, queue_version } => {
                RunMutation::Complete(Run::from_wire(*run)?, queue_version)
            }
            StoreRequest::DrainQueue { run } => RunMutation::DrainQueue(Run::from_wire(*run)?),
        };
        let receipt = self
            .store
            .commit_worker(
                &WorkerLease {
                    run_id: id,
                    instance_id: lease.worker_instance_id.clone(),
                    epoch: lease.lease_epoch,
                },
                operation_id,
                mutation,
            )
            .await
            .map_err(rejected)?;
        Ok(match receipt.completed {
            Some(completed) => StoreResponse::Completion {
                run: Box::new(receipt.run.to_wire()),
                completed,
            },
            None => StoreResponse::Run {
                run: Box::new(receipt.run.to_wire()),
            },
        })
    }
}

fn validate_wire_message_tool_inputs(
    value: &ait_contracts::worker::model::Message,
) -> Result<(), ProtocolError> {
    for part in &value.sub_messages {
        if let ait_contracts::worker::model::SubMessage::ToolUse(tool) = part {
            ait_contracts::sensitive::validate_serialized_tool_arguments(&tool.arguments)
                .map_err(|_| ProtocolError::InvalidTransition)?;
        }
    }
    Ok(())
}
fn validate_wire_tool_input(
    value: &ait_contracts::worker::model::ToolExecution,
) -> Result<(), ProtocolError> {
    ait_contracts::sensitive::validate_tool_argument_value(&value.arguments)
        .map_err(|_| ProtocolError::InvalidTransition)
}

fn validate_private_input(request: &StoreRequest) -> Result<(), ProtocolError> {
    match request {
        StoreRequest::AppendMessage { message: value, .. } => {
            validate_wire_message_tool_inputs(value)
        }
        StoreRequest::SaveTool { tool: value, .. } => validate_wire_tool_input(value),
        StoreRequest::ToolInteraction { request } => {
            ait_contracts::sensitive::validate_tool_argument_value(&request.arguments)
                .map_err(|_| ProtocolError::InvalidTransition)
        }
        StoreRequest::ToolInteractionRecovery { execution } => validate_wire_tool_input(execution),
        StoreRequest::AppendToolResult {
            tool: execution,
            message: result,
            ..
        } => {
            validate_wire_tool_input(execution)?;
            validate_wire_message_tool_inputs(result)
        }
        _ => Ok(()),
    }
}
fn page<T>(entries: Vec<T>, offset: u32) -> Result<(Vec<T>, Option<u32>), ProtocolError> {
    let offset = offset as usize;
    if offset > entries.len() {
        return Err(ProtocolError::InvalidFrame);
    }
    let next = (entries.len() > offset + 1).then(|| u32::try_from(offset + 1).unwrap_or(u32::MAX));
    Ok((entries.into_iter().skip(offset).take(1).collect(), next))
}

#[cfg(test)]
mod private_input_tests;
