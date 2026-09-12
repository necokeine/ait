//! Codex ports transported by the same private worker supervisor.
use crate::{
    mapping::Wire,
    supervisor::{Handler, WorkerSupervisor},
};
use ait_contracts::worker::{
    Bootstrap, Executor, Lease, ProtocolError, StoreRequest, StoreResponse, WorkspaceInvocation,
};
use ait_domain::{DomainError, ErrorCode};
use ait_ports::{
    WorkspaceAgent, WorkspaceAgentInvocation, WorkspaceAgentResponse, WorkspaceProgressReporter,
    WorkspaceResultSink,
};
use async_trait::async_trait;
use sha2::Digest;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

type MemoValue = Option<Result<StoreResponse, ProtocolError>>;
type Memo = ([u8; 32], tokio::sync::watch::Receiver<MemoValue>);

fn failed() -> DomainError {
    DomainError::invariant(
        ErrorCode::RunRecoveryFailed,
        "workspace worker stopped; inspect durable recovery state",
    )
}
struct WorkspaceServer<'a> {
    lease: Lease,
    invocation: &'a WorkspaceAgentInvocation,
    progress: Option<Arc<dyn WorkspaceProgressReporter>>,
    sink: Option<&'a dyn WorkspaceResultSink>,
    checkpoint: Mutex<Option<WorkspaceAgentResponse>>,
    result: Mutex<Option<WorkspaceAgentResponse>>,
    receipts: Mutex<HashMap<String, Memo>>,
}
#[async_trait]
impl Handler for WorkspaceServer<'_> {
    fn capacity(&self) -> usize {
        16
    }
    fn disconnected(&self) {
        self.invocation.cancellation.cancel();
    }
    async fn request(
        &self,
        lease: &Lease,
        operation_id: &str,
        request: StoreRequest,
    ) -> Result<StoreResponse, ProtocolError> {
        if lease != &self.lease {
            return Err(ProtocolError::StaleWorkerLease);
        }
        let fingerprint: [u8; 32] = sha2::Sha256::digest(
            serde_json::to_vec(&request).map_err(|_| ProtocolError::InvalidFrame)?,
        )
        .into();
        let (sender, mut receiver) = {
            let mut receipts = self.receipts.lock().map_err(|_| ProtocolError::Io)?;
            if let Some((prior, receiver)) = receipts.get(operation_id) {
                if prior != &fingerprint {
                    return Err(ProtocolError::OperationConflict);
                }
                (None, receiver.clone())
            } else {
                if receipts.len() >= 8192 {
                    return Err(ProtocolError::ResourceLimit);
                }
                let (sender, receiver) = tokio::sync::watch::channel(None);
                receipts.insert(operation_id.into(), (fingerprint, receiver.clone()));
                (Some(sender), receiver)
            }
        };
        if let Some(sender) = sender {
            let result = self.apply(lease, operation_id, request).await;
            let _ = sender.send(Some(result.clone()));
            result
        } else {
            loop {
                if let Some(result) = receiver.borrow().clone() {
                    return result;
                }
                receiver.changed().await.map_err(|_| ProtocolError::Io)?;
            }
        }
    }
    async fn finished(&self) -> Result<(), ProtocolError> {
        if self.result.lock().map_err(|_| ProtocolError::Io)?.is_some() {
            Ok(())
        } else {
            Err(ProtocolError::InvalidTransition)
        }
    }
}
impl WorkspaceServer<'_> {
    async fn apply(
        &self,
        lease: &Lease,
        operation_id: &str,
        request: StoreRequest,
    ) -> Result<StoreResponse, ProtocolError> {
        let operation = ait_ports::WorkspaceWorkerOperation {
            lease: ait_ports::WorkerLease {
                run_id: ait_domain::RunId::new(&lease.run_id),
                instance_id: lease.worker_instance_id.clone(),
                epoch: lease.lease_epoch,
            },
            operation_id: operation_id.into(),
        };
        match request {
            StoreRequest::WorkspaceProgress { event } => {
                if let Some(progress) = &self.progress {
                    progress
                        .report(ait_ports::WorkspaceProgressEvent::from_wire(*event)?)
                        .await;
                }
            }
            StoreRequest::WorkspaceCheckpoint { result } => {
                let result = WorkspaceAgentResponse::from_wire(*result)?;
                self.sink
                    .ok_or(ProtocolError::InvalidTransition)?
                    .checkpoint_worker(&operation, result.clone())
                    .await
                    .map_err(|_| ProtocolError::InvalidTransition)?;
                *self.checkpoint.lock().map_err(|_| ProtocolError::Io)? = Some(result);
            }
            StoreRequest::WorkspaceIntegration => {
                if self
                    .checkpoint
                    .lock()
                    .map_err(|_| ProtocolError::Io)?
                    .is_none()
                {
                    return Err(ProtocolError::InvalidTransition);
                }
                self.invocation
                    .integration_gate
                    .as_ref()
                    .ok_or(ProtocolError::InvalidTransition)?
                    .begin_worker_integration(&operation)
                    .await
                    .map_err(|_| ProtocolError::InvalidTransition)?;
            }
            StoreRequest::WorkspaceApproval { request, expire } => {
                let request = ait_ports::WorkspaceApprovalRequest::from_wire(*request)?;
                if request.run_id != lease.run_id {
                    return Err(ProtocolError::WrongRun);
                }
                if expire {
                    self.invocation
                        .approvals
                        .expire(&request)
                        .await
                        .map_err(|_| ProtocolError::InvalidTransition)?;
                } else {
                    return Ok(StoreResponse::WorkspaceApproval {
                        decision: self
                            .invocation
                            .approvals
                            .decide(request)
                            .await
                            .map_err(|_| ProtocolError::InvalidTransition)?
                            .to_wire(),
                    });
                }
            }
            StoreRequest::WorkspaceFinished { result } => {
                let result = WorkspaceAgentResponse::from_wire(*result)?;
                if self
                    .checkpoint
                    .lock()
                    .map_err(|_| ProtocolError::Io)?
                    .as_ref()
                    != Some(&result)
                {
                    return Err(ProtocolError::InvalidTransition);
                }
                *self.result.lock().map_err(|_| ProtocolError::Io)? = Some(result);
            }
            _ => return Err(ProtocolError::InvalidFrame),
        }
        Ok(StoreResponse::Unit)
    }
}
impl WorkerSupervisor {
    async fn workspace(
        &self,
        request: WorkspaceAgentInvocation,
        progress: Option<Arc<dyn WorkspaceProgressReporter>>,
        sink: Option<&dyn WorkspaceResultSink>,
        recovery: Option<WorkspaceAgentResponse>,
        baseline_ref: Option<String>,
    ) -> Result<WorkspaceAgentResponse, DomainError> {
        let identity = request
            .integration_gate
            .as_ref()
            .ok_or_else(failed)?
            .claim_worker(&uuid::Uuid::new_v4().to_string())
            .await?;
        let lease = Lease {
            run_id: identity.run_id.as_str().into(),
            worker_instance_id: identity.instance_id,
            lease_epoch: identity.epoch,
        };
        let bootstrap = Bootstrap {
            lease: lease.clone(),
            limits: self.limits.clone(),
            workdir: request.cwd.to_string_lossy().into_owned(),
            permission: request.permission_profile.to_wire(),
            maximum_sandbox: request.permission_profile.sandbox.to_wire(),
            executor: Executor::Workspace {
                invocation: Box::new(WorkspaceInvocation {
                    codex_binary: self.codex_binary.to_string_lossy().into_owned(),
                    request_id: request.request_id.clone(),
                    model: request.model.clone(),
                    reasoning_effort: request.reasoning_effort.clone(),
                    project_instructions: request.project_instructions.clone(),
                    prompt: request.prompt.clone(),
                    commit_subject: request.commit_subject.clone(),
                    baseline_commit: request.baseline_commit.clone(),
                    baseline_index_tree: request.baseline_index_tree.clone(),
                    recovery_result: recovery.to_wire(),
                    baseline_ref,
                }),
            },
        };
        let server = WorkspaceServer {
            lease,
            invocation: &request,
            progress,
            sink,
            checkpoint: Mutex::new(recovery),
            result: Mutex::new(None),
            receipts: Mutex::new(HashMap::new()),
        };
        self.process(bootstrap, &server, request.cancellation.clone())
            .await
            .map_err(|_| failed())?;
        server
            .result
            .lock()
            .map_err(|_| failed())?
            .take()
            .ok_or_else(failed)
    }
}
#[async_trait]
impl WorkspaceAgent for WorkerSupervisor {
    async fn invoke(
        &self,
        _request: WorkspaceAgentInvocation,
    ) -> Result<WorkspaceAgentResponse, DomainError> {
        Err(failed())
    }
    async fn invoke_with_progress_and_checkpoint(
        &self,
        request: WorkspaceAgentInvocation,
        progress: Arc<dyn WorkspaceProgressReporter>,
        sink: &dyn WorkspaceResultSink,
    ) -> Result<WorkspaceAgentResponse, DomainError> {
        self.workspace(request, Some(progress), Some(sink), None, None)
            .await
    }
    async fn recover_checkpointed(
        &self,
        request: WorkspaceAgentInvocation,
        result: WorkspaceAgentResponse,
        baseline_ref: Option<String>,
    ) -> Result<WorkspaceAgentResponse, DomainError> {
        self.workspace(request, None, None, Some(result), baseline_ref)
            .await
    }
}
