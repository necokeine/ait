//! Codex ports backed exclusively by supervised worker processes.
use std::sync::{Arc, Mutex};

use ait_contracts::worker::{
    Bootstrap, Executor, Lease, ProtocolError, StoreRequest, StoreResponse,
    codex::{Action, MAX_RESULT_BYTES, Operation},
};
use ait_domain::{AgentProvider, DomainError, ErrorCode, ProviderModel, RunPermissionProfile};
use ait_ports::{
    CodexHistorySource, CodexPreparedThread, CodexThreadConnection, CodexThreadInvocation,
    CodexThreadSnapshot, CodexThreadSourceKind, CodexThreadWriter, GeneratedSessionTitle,
    HostProviderModelCatalog, SessionTitleGenerator, SessionTitleRequest, WorkspaceApproval,
    WorkspaceProgressReporter,
};
use async_trait::async_trait;
use serde::de::DeserializeOwned;
use serde_json::Value;
use tokio::sync::{Mutex as AsyncMutex, mpsc};
use tokio_util::sync::CancellationToken;

use crate::{
    mapping::Wire,
    supervisor::{Handler, WorkerSupervisor},
};

type Reply = Result<(Value, bool), DomainError>;

fn failed() -> DomainError {
    DomainError::invariant(
        ErrorCode::CodexInputOutcomeUnknown,
        "Codex worker disconnected; reconcile native history before sending more input",
    )
}

#[derive(Default)]
struct Assembly {
    bytes: Vec<u8>,
    total: usize,
    closed: bool,
}

struct Server {
    project_execution: Option<Arc<dyn ait_ports::ProjectExecution>>,
    lease: Lease,
    request_id: Option<String>,
    assembly: Mutex<Assembly>,
    actions: AsyncMutex<mpsc::Receiver<Action>>,
    replies: mpsc::Sender<Reply>,
    progress: Arc<Mutex<Option<Arc<dyn WorkspaceProgressReporter>>>>,
    approvals: Option<Arc<dyn WorkspaceApproval>>,
    cancel: CancellationToken,
}

#[async_trait]
impl Handler for Server {
    async fn spawned(&self, pid: u32) -> Result<(), ProtocolError> {
        if let Some(project) = &self.project_execution {
            project
                .register_process(pid)
                .await
                .map_err(|_| ProtocolError::StaleWorkerLease)?;
        }
        Ok(())
    }
    async fn reaped(&self, pid: u32) -> Result<(), ProtocolError> {
        if let Some(project) = &self.project_execution {
            project
                .release_process(pid)
                .await
                .map_err(|_| ProtocolError::StaleWorkerLease)?;
        }
        Ok(())
    }
    fn capacity(&self) -> usize {
        16
    }

    fn disconnected(&self) {
        self.cancel.cancel();
    }

    async fn request(
        &self,
        lease: &Lease,
        _operation_id: &str,
        request: StoreRequest,
    ) -> Result<StoreResponse, ProtocolError> {
        if lease != &self.lease {
            return Err(ProtocolError::StaleWorkerLease);
        }
        match request {
            StoreRequest::CodexChunk {
                offset,
                total,
                bytes,
            } => {
                let result = {
                    let mut assembly = self.assembly.lock().map_err(|_| ProtocolError::Io)?;
                    if assembly.closed
                        || total == 0
                        || total > MAX_RESULT_BYTES
                        || offset != assembly.bytes.len()
                        || bytes.is_empty()
                        || bytes.len() > ait_contracts::worker::codex::CHUNK_BYTES
                        || offset
                            .checked_add(bytes.len())
                            .is_none_or(|end| end > total)
                        || (offset > 0 && total != assembly.total)
                    {
                        return Err(ProtocolError::InvalidFrame);
                    }
                    assembly.total = total;
                    assembly.bytes.extend_from_slice(&bytes);
                    if assembly.bytes.len() == total {
                        let reply = serde_json::from_slice::<Reply>(&assembly.bytes)
                            .map_err(|_| ProtocolError::InvalidFrame)?;
                        assembly.bytes.clear();
                        Some(reply)
                    } else {
                        None
                    }
                };
                if let Some(reply) = result {
                    self.replies
                        .send(reply)
                        .await
                        .map_err(|_| ProtocolError::Io)?;
                }
                Ok(StoreResponse::Unit)
            }
            StoreRequest::CodexNext => {
                let mut receiver = self.actions.lock().await;
                let action = tokio::select! {
                    action = receiver.recv() => action.unwrap_or(Action::Close),
                    () = self.cancel.cancelled() => Action::Close,
                };
                Ok(StoreResponse::CodexAction { action })
            }
            StoreRequest::CodexClosed => {
                let mut assembly = self.assembly.lock().map_err(|_| ProtocolError::Io)?;
                if !assembly.bytes.is_empty() {
                    return Err(ProtocolError::InvalidTransition);
                }
                assembly.closed = true;
                Ok(StoreResponse::Unit)
            }
            StoreRequest::WorkspaceProgress { event } => {
                let progress = self.progress.lock().map_err(|_| ProtocolError::Io)?.clone();
                if let Some(progress) = progress {
                    progress
                        .report(ait_ports::WorkspaceProgressEvent::from_wire(*event)?)
                        .await;
                }
                Ok(StoreResponse::Unit)
            }
            StoreRequest::WorkspaceApproval { request, expire } => {
                let request = ait_ports::WorkspaceApprovalRequest::from_wire(*request)?;
                if self.request_id.as_deref() != Some(request.run_id.as_str()) {
                    return Err(ProtocolError::WrongRun);
                }
                let approvals = self
                    .approvals
                    .as_ref()
                    .ok_or(ProtocolError::InvalidTransition)?;
                if expire {
                    approvals
                        .expire(&request)
                        .await
                        .map_err(|_| ProtocolError::InvalidTransition)?;
                    Ok(StoreResponse::Unit)
                } else {
                    let decision = tokio::select! {
                        result = approvals.decide(request) => result.map_err(|_| ProtocolError::InvalidTransition)?,
                        () = self.cancel.cancelled() => return Err(ProtocolError::WorkerExited),
                    };
                    Ok(StoreResponse::WorkspaceApproval {
                        decision: decision.to_wire(),
                    })
                }
            }
            _ => Err(ProtocolError::InvalidFrame),
        }
    }

    async fn finished(&self) -> Result<(), ProtocolError> {
        if self.assembly.lock().map_err(|_| ProtocolError::Io)?.closed {
            Ok(())
        } else {
            Err(ProtocolError::InvalidTransition)
        }
    }
}

struct RemoteConnection {
    resumed: Option<CodexPreparedThread>,
    actions: mpsc::Sender<Action>,
    replies: mpsc::Receiver<Reply>,
    progress: Arc<Mutex<Option<Arc<dyn WorkspaceProgressReporter>>>>,
    cancel: CancellationToken,
    worker: Option<tokio::task::JoinHandle<()>>,
}

impl Drop for RemoteConnection {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

impl RemoteConnection {
    async fn receive<T: DeserializeOwned>(&mut self) -> Result<(T, bool), DomainError> {
        let (value, owned) = self.replies.recv().await.ok_or_else(failed)??;
        serde_json::from_value(value)
            .map(|value| (value, owned))
            .map_err(|_| failed())
    }

    async fn history(&mut self, action: Action) -> Result<CodexThreadSnapshot, DomainError> {
        self.actions.send(action).await.map_err(|_| failed())?;
        let (mut history, owned): (CodexThreadSnapshot, bool) = self.receive().await?;
        history.writer_confirmed = owned;
        Ok(history)
    }
}

#[async_trait]
impl CodexThreadConnection for RemoteConnection {
    fn prepared(&self) -> &CodexPreparedThread {
        self.resumed
            .as_ref()
            .expect("writer exposed only after prepared response")
    }

    async fn start(
        &mut self,
        progress: Arc<dyn WorkspaceProgressReporter>,
    ) -> Result<CodexThreadSnapshot, DomainError> {
        *self.progress.lock().map_err(|_| failed())? = Some(progress);
        self.history(Action::Start).await
    }

    async fn read(&mut self) -> Result<CodexThreadSnapshot, DomainError> {
        self.history(Action::Read).await
    }

    async fn close(&mut self) {
        let _ = self.actions.send(Action::Close).await;
        if let Some(worker) = self.worker.take() {
            let _ = worker.await;
        }
    }
}

impl WorkerSupervisor {
    fn codex_connection(
        &self,
        operation: Operation,
        cwd: String,
        permission: RunPermissionProfile,
        invocation: Option<&CodexThreadInvocation>,
    ) -> RemoteConnection {
        let identity = uuid::Uuid::new_v4().to_string();
        let lease = Lease {
            project_owner: invocation.and_then(|request| {
                request
                    .project_execution
                    .as_ref()
                    .map(|project| project.owner())
            }),
            scope_id: format!("codex:{identity}"),
            worker_instance_id: identity,
            lease_epoch: 1,
        };
        let cancel = invocation.map_or_else(CancellationToken::new, |request| {
            request.cancellation.child_token()
        });
        let (actions, receiver) = mpsc::channel(1);
        let (sender, replies) = mpsc::channel(2);
        let progress = Arc::new(Mutex::new(None));
        let server = Server {
            project_execution: invocation.and_then(|request| request.project_execution.clone()),
            lease: lease.clone(),
            request_id: invocation.map(|request| request.request_id.clone()),
            assembly: Mutex::new(Assembly::default()),
            actions: AsyncMutex::new(receiver),
            replies: sender.clone(),
            progress: progress.clone(),
            approvals: invocation.map(|request| request.approvals.clone()),
            cancel: cancel.clone(),
        };
        let bootstrap = Bootstrap {
            lease,
            limits: self.limits.clone(),
            workdir: cwd,
            maximum_sandbox: permission.sandbox.to_wire(),
            permission: permission.to_wire(),
            executor: Executor::Codex {
                binary: self.codex_binary.to_string_lossy().into_owned(),
                operation: Box::new(operation),
            },
        };
        let supervisor = self.clone();
        let worker_cancel = cancel.clone();
        let worker = tokio::spawn(async move {
            if let Err(failure) = supervisor.process(bootstrap, &server, worker_cancel).await {
                let mut error = failed();
                error.message = format!("{} ({failure})", error.message);
                if failure == ProtocolError::ResourceLimit {
                    error.code = ErrorCode::RunLimitExceeded;
                }
                let _ = sender.send(Err(error)).await;
            }
        });
        RemoteConnection {
            resumed: None,
            actions,
            replies,
            progress,
            cancel,
            worker: Some(worker),
        }
    }

    async fn codex_query<T: DeserializeOwned>(
        &self,
        operation: Operation,
        cwd: String,
        cancellation: CancellationToken,
    ) -> Result<T, DomainError> {
        let mut connection =
            self.codex_connection(operation, cwd, RunPermissionProfile::default(), None);
        let result = tokio::select! {
            result = connection.receive::<T>() => result.map(|(value, _)| value),
            () = cancellation.cancelled() => { connection.cancel.cancel(); Err(failed()) },
        };
        connection.close().await;
        result
    }
}

#[async_trait]
impl CodexThreadWriter for WorkerSupervisor {
    async fn open(
        &self,
        request: CodexThreadInvocation,
    ) -> Result<Box<dyn CodexThreadConnection>, DomainError> {
        let operation = Operation::Open {
            request_id: request.request_id.clone(),
            thread_id: request.thread_id.clone(),
            prompt: request.prompt.clone(),
            model: request.model.clone(),
            reasoning_effort: request.reasoning_effort.clone(),
            developer_instructions: request.developer_instructions.clone(),
        };
        let mut connection = self.codex_connection(
            operation,
            request.cwd.to_string_lossy().into_owned(),
            request.permission_profile,
            Some(&request),
        );
        match connection.receive::<CodexPreparedThread>().await {
            Ok((mut resumed, owned)) => {
                resumed.history.writer_confirmed = owned;
                connection.resumed = Some(resumed);
                Ok(Box::new(connection))
            }
            Err(error) => {
                connection.close().await;
                Err(error)
            }
        }
    }
}

fn host_cwd() -> Result<String, DomainError> {
    std::env::current_dir()
        .map(|path| path.to_string_lossy().into_owned())
        .map_err(|_| failed())
}

#[async_trait]
impl CodexHistorySource for WorkerSupervisor {
    async fn list_threads(
        &self,
        source_kinds: &[CodexThreadSourceKind],
    ) -> Result<Vec<CodexThreadSnapshot>, DomainError> {
        let source_kinds =
            serde_json::from_value(serde_json::to_value(source_kinds).map_err(|_| failed())?)
                .map_err(|_| failed())?;
        self.codex_query(
            Operation::List { source_kinds },
            host_cwd()?,
            CancellationToken::new(),
        )
        .await
    }

    async fn read_thread(&self, thread_id: &str) -> Result<CodexThreadSnapshot, DomainError> {
        self.codex_query(
            Operation::Read {
                thread_id: thread_id.into(),
            },
            host_cwd()?,
            CancellationToken::new(),
        )
        .await
    }
}

#[async_trait]
impl HostProviderModelCatalog for WorkerSupervisor {
    async fn discover_models(
        &self,
        provider: &AgentProvider,
    ) -> Result<Vec<ProviderModel>, DomainError> {
        self.codex_query(
            Operation::Models {
                provider: provider.clone(),
            },
            host_cwd()?,
            CancellationToken::new(),
        )
        .await
    }
}

#[async_trait]
impl SessionTitleGenerator for WorkerSupervisor {
    async fn generate(
        &self,
        request: SessionTitleRequest,
    ) -> Result<GeneratedSessionTitle, DomainError> {
        self.codex_query(
            Operation::Title {
                request_id: request.request_id,
                user_prompt: request.user_prompt,
                config: request.config,
                provider: request.provider,
            },
            request.cwd.to_string_lossy().into_owned(),
            request.cancellation,
        )
        .await
    }
}

#[cfg(test)]
mod tests;
