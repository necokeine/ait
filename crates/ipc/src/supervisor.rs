//! One supervised process per API Run. Durable state decides success.
use crate::{
    codec::{Reader, Writer},
    connection::Connection,
    mapping::Wire,
    rpc::StoreServer,
};
use ait_contracts::worker::{
    Bootstrap, CredentialGrant, Executor, Lease, Limits, MAX_FRAME_BYTES, Payload, ProtocolError,
};
use ait_domain::{DomainError, ErrorCode, ProviderKind};
use ait_ports::{ApiRunDispatch, RunDispatcher};
use async_trait::async_trait;
use futures_util::{StreamExt, future::BoxFuture, stream::FuturesUnordered};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

/// Shared production dispatcher; entries exist only while owned children run.
pub struct WorkerSupervisor {
    binary: PathBuf,
    pub(crate) codex_binary: PathBuf,
    pub(crate) limits: Limits,
    active: Mutex<HashMap<String, CancellationToken>>,
    draining: CancellationToken,
    observer: Option<Arc<dyn WorkerObserver>>,
}

/// Observable commit barrier used by diagnostics and deterministic fault injection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CommitBoundary {
    /// Before a received mutation enters the durable writer.
    BeforeCommit,
    /// Durable commit finished; receipt has not been sent.
    BeforeAck,
    /// Receipt bytes were flushed to the worker pipe.
    AfterAck,
}
/// Bounded observation surface. Never receives credentials or tool arguments.
pub trait WorkerObserver: Send + Sync {
    /// Observe a frame class at a commit boundary. Returning an error stops that request.
    /// # Errors
    /// A diagnostic/fault hook may explicitly interrupt the attempt.
    fn checkpoint(
        &self,
        pid: u32,
        method: &str,
        boundary: CommitBoundary,
    ) -> Result<(), ProtocolError>;
}
impl WorkerSupervisor {
    /// Use an explicitly configured, trusted worker binary.
    #[must_use]
    pub fn new(binary: PathBuf) -> Self {
        Self {
            binary,
            codex_binary: PathBuf::from("codex"),
            limits: Limits::default(),
            active: Mutex::new(HashMap::new()),
            draining: CancellationToken::new(),
            observer: None,
        }
    }
    /// Pin the native harness executable without credentials or environment grants.
    #[must_use]
    pub fn with_codex_binary(mut self, binary: PathBuf) -> Self {
        self.codex_binary = binary;
        self
    }
    /// Enable a strict optional monetary ceiling. Unpriced providers fail closed.
    #[must_use]
    pub fn with_cost_ceiling(mut self, max_cost_micros: Option<u64>) -> Self {
        self.limits.max_cost_micros = max_cost_micros;
        self
    }
    /// Install an observer without changing the protocol or durable state machine.
    #[must_use]
    pub fn with_observer(mut self, observer: Arc<dyn WorkerObserver>) -> Self {
        self.observer = Some(observer);
        self
    }
    /// Stop admitting work and request cooperative drain of every owned worker.
    pub fn drain(&self) {
        self.draining.cancel();
    }

    /// Run an already-claimed bootstrap, useful for offline process acceptance tests.
    /// # Errors
    /// Process and protocol failures never contain child stdout/stderr text.
    pub async fn execute(
        &self,
        bootstrap: Bootstrap,
        store: Arc<dyn ait_ports::RunStore>,
        cancel: CancellationToken,
    ) -> Result<(), ProtocolError> {
        let server = StoreServer {
            store,
            lease: bootstrap.lease.clone(),
        };

        self.process(bootstrap, &server, cancel).await
    }
    pub(crate) async fn process(
        &self,
        mut bootstrap: Bootstrap,
        server: &dyn Handler,
        cancel: CancellationToken,
    ) -> Result<(), ProtocolError> {
        bootstrap.limits.validate()?;
        let run_id = bootstrap.lease.run_id.clone();
        {
            let mut active = self.active.lock().map_err(|_| ProtocolError::Io)?;
            if self.draining.is_cancelled() {
                return Err(ProtocolError::Draining);
            }
            if active.contains_key(&run_id) {
                return Err(ProtocolError::InvalidTransition);
            }
            if active.len() >= 16 {
                return Err(ProtocolError::ResourceLimit);
            }
            active.insert(run_id.clone(), cancel.clone());
        }
        let _active = ActiveGuard {
            supervisor: self,
            run_id,
        };
        let mut child =
            ait_sandbox::spawn_worker(&self.binary).map_err(|_| ProtocolError::WorkerExited)?;
        let result = async {
            let pid = child.id().ok_or(ProtocolError::WorkerExited)?;
            let mut reader = Reader::new(
                child.stdout().take().ok_or(ProtocolError::Io)?,
                MAX_FRAME_BYTES,
            );
            let mut writer = Writer::new(
                child.stdin().take().ok_or(ProtocolError::Io)?,
                MAX_FRAME_BYTES,
            );
            tokio::time::timeout(Duration::from_secs(3), async {
                let frame = reader.read().await?;
                let Payload::Hello(hello) = frame.payload else {
                    return Err(ProtocolError::InvalidFrame);
                };
                hello.validate()?;
                if frame.lease.is_some() || hello.pid != pid {
                    return Err(ProtocolError::InvalidFrame);
                }
                bootstrap.limits.max_frame_bytes =
                    bootstrap.limits.max_frame_bytes.min(hello.max_frame_bytes);
                reader.constrain(bootstrap.limits.max_frame_bytes);
                writer.constrain(bootstrap.limits.max_frame_bytes);
                writer.write(None, Payload::HelloAck).await?;
                writer
                    .write(
                        Some(bootstrap.lease.clone()),
                        Payload::Bootstrap(Box::new(bootstrap.clone())),
                    )
                    .await?;
                let frame = reader.read().await?;
                if frame.lease.as_ref() != Some(&bootstrap.lease)
                    || !matches!(frame.payload,Payload::Ready{pid:ready} if ready==pid)
                {
                    return Err(ProtocolError::InvalidFrame);
                }
                Ok(())
            })
            .await
            .map_err(|_| ProtocolError::HandshakeTimeout)??;
            let secret = match &bootstrap.executor {
                Executor::Api { credential, .. } => Some(credential.0.clone()),
                _ => None,
            };
            let mut pipe = Connection::start(reader, writer, bootstrap.lease.clone(), secret);
            let outcome = self.serve(&mut pipe, server, &bootstrap, cancel, pid).await;
            pipe.close().await;
            outcome
        }
        .await;
        // Also kill descendants after a clean leader exit. Never trust exit_report
        // as proof that a provider left no background child behind.
        let _ = child.start_kill();
        let _ = tokio::time::timeout(Duration::from_secs(3), child.wait()).await;
        result
    }
    async fn serve(
        &self,
        pipe: &mut Connection,
        server: &dyn Handler,
        bootstrap: &Bootstrap,
        cancel: CancellationToken,
        pid: u32,
    ) -> Result<(), ProtocolError> {
        let mut heartbeat = Instant::now();
        let mut interval =
            tokio::time::interval(Duration::from_millis(bootstrap.limits.heartbeat_ms));
        let deadline = Instant::now() + Duration::from_millis(bootstrap.limits.wall_clock_ms);
        let mut drain_deadline = None;
        let mut last_request_id = 0;
        let mut pending: FuturesUnordered<BoxFuture<'_, Reply>> = FuturesUnordered::new();
        let result=async {loop {
            tokio::select! {
                biased;
                Some(result)=pending.next()=>{
                    let (request_id,operation_id,method,result)=result;
                    if let Some(observer)=&self.observer {observer.checkpoint(pid,method,CommitBoundary::BeforeAck)?;}
                    let payload=match result {Ok(response)=>Payload::Receipt{request_id,operation_id,response:Box::new(response)},Err(code)=>Payload::Rejected{request_id,code}};
                    pipe.send(payload).await?;
                    if let Some(observer)=&self.observer {observer.checkpoint(pid,method,CommitBoundary::AfterAck)?;}
                }
                frame=pipe.receiver.recv()=>{
                    let frame=frame.ok_or(ProtocolError::UnexpectedEof)??;
                    if frame.lease.as_ref()!=Some(&bootstrap.lease){return Err(ProtocolError::StaleWorkerLease)}
                    heartbeat=Instant::now();
                    match frame.payload {
                        Payload::Heartbeat=>{},
                        Payload::Request{request_id,operation_id,request}=>{
                            if request_id <= last_request_id { return Err(ProtocolError::CorrelationMismatch); }
                            if operation_id.is_empty() || operation_id.len() > 256 { return Err(ProtocolError::OperationConflict); }
                            last_request_id = request_id;
                            if pending.len()>=server.capacity(){return Err(ProtocolError::ResourceLimit)}
                            let lease=bootstrap.lease.clone();
                            let method=request_method(&request);
                            if let Some(observer)=&self.observer {observer.checkpoint(pid,method,CommitBoundary::BeforeCommit)?;}
                            pending.push(Box::pin(async move {let result=server.request(&lease,&operation_id,*request).await;(request_id,operation_id,method,result)}));
                        },
                        Payload::ExitReport=>{
                            if !pending.is_empty(){return Err(ProtocolError::InvalidTransition)}
                            return server.finished().await;
                        },
                        _=>return Err(ProtocolError::InvalidFrame),
                    }
                }
                _=interval.tick()=>{
                    let now=Instant::now();
                    if now.duration_since(heartbeat)>Duration::from_millis(bootstrap.limits.heartbeat_timeout_ms){return Err(ProtocolError::HeartbeatTimeout)}
                    if drain_deadline.is_some_and(|d|now>=d){return Err(ProtocolError::WorkerExited)}
                    if drain_deadline.is_none()&&(cancel.is_cancelled()||self.draining.is_cancelled()||now>=deadline){
                        pipe.send(Payload::Cancel).await?;
                        drain_deadline=Some(now+Duration::from_millis(bootstrap.limits.drain_ms));
                    }
                    pipe.send(Payload::Heartbeat).await?;
                }
            }
        }}.await;
        // A commit already entering the store must finish before the next lease.
        if result.is_err() {
            server.disconnected();
        }
        while pending.next().await.is_some() {}
        result
    }
}
#[async_trait]
impl RunDispatcher for WorkerSupervisor {
    #[allow(
        clippy::match_wildcard_for_single_variants,
        reason = "non-API development Provider variants must fail closed"
    )]
    async fn dispatch(&self, request: ApiRunDispatch) -> Result<(), DomainError> {
        let failure = || {
            DomainError::invariant(
                ErrorCode::RunRecoveryFailed,
                "supervised worker stopped; durable state requires recovery",
            )
        };
        let provider = match request.provider.kind {
            ProviderKind::OpenAI => "openai",
            ProviderKind::DeepSeek => "deepseek",
            #[allow(clippy::match_wildcard_for_single_variants)]
            _ => return Err(failure()),
        };
        for _ in 0..3 {
            let lease = request
                .store
                .claim_worker(&uuid::Uuid::new_v4().to_string())
                .await
                .map_err(|_| failure())?;
            if lease.run_id != request.run_id {
                return Err(failure());
            }
            let bootstrap = Bootstrap {
                lease: Lease {
                    run_id: lease.run_id.as_str().into(),
                    worker_instance_id: lease.instance_id,
                    lease_epoch: lease.epoch,
                },
                limits: self.limits.clone(),
                workdir: request.workdir.to_string_lossy().into_owned(),
                permission: request.permission.to_wire(),
                maximum_sandbox: request.maximum_sandbox.to_wire(),
                executor: Executor::Api {
                    provider: provider.into(),
                    endpoint: request.provider.url.clone(),
                    model: request.config.model.clone(),
                    reasoning_effort: request.config.reasoning_effort.clone(),
                    credential: CredentialGrant(request.credential.clone()),
                },
            };
            let result = self
                .execute(
                    bootstrap,
                    request.store.clone(),
                    request.cancellation.clone(),
                )
                .await;
            // Includes a lost terminal ACK: authoritative state wins over EOF.
            let run = request
                .store
                .load_run(&request.run_id)
                .await
                .map_err(|_| failure())?;
            if run.status.is_terminal() || result.is_ok() {
                return Ok(());
            }
            if request.cancellation.is_cancelled() || self.draining.is_cancelled() {
                return Err(failure());
            }
            // RunCoordinator resumes known results and reconciles unknown tools;
            // no second business lifecycle or blind side-effect retry is added.
        }
        Err(failure())
    }
}

/// Daemon ports serviced by one worker connection.
#[async_trait]
pub(crate) trait Handler: Send + Sync {
    fn capacity(&self) -> usize {
        1
    }
    fn disconnected(&self) {}
    async fn request(
        &self,
        lease: &Lease,
        operation_id: &str,
        request: ait_contracts::worker::StoreRequest,
    ) -> Result<ait_contracts::worker::StoreResponse, ProtocolError>;
    async fn finished(&self) -> Result<(), ProtocolError>;
}
#[async_trait]
impl Handler for StoreServer {
    async fn request(
        &self,
        lease: &Lease,
        operation_id: &str,
        request: ait_contracts::worker::StoreRequest,
    ) -> Result<ait_contracts::worker::StoreResponse, ProtocolError> {
        self.handle(lease, operation_id, request).await
    }
    async fn finished(&self) -> Result<(), ProtocolError> {
        let run = self
            .store
            .load_run(&ait_domain::RunId::new(&self.lease.run_id))
            .await
            .map_err(|_| ProtocolError::InvalidTransition)?;
        if run.status.is_terminal() || run.status == ait_domain::RunStatus::WaitingApproval {
            Ok(())
        } else {
            Err(ProtocolError::WorkerExited)
        }
    }
}

struct ActiveGuard<'a> {
    supervisor: &'a WorkerSupervisor,
    run_id: String,
}
impl Drop for ActiveGuard<'_> {
    fn drop(&mut self) {
        if let Ok(mut active) = self.supervisor.active.lock() {
            active.remove(&self.run_id);
        }
    }
}
type Reply = (
    u64,
    String,
    &'static str,
    Result<ait_contracts::worker::StoreResponse, ProtocolError>,
);
fn request_method(request: &ait_contracts::worker::StoreRequest) -> &'static str {
    use ait_contracts::worker::{StoreRequest, model::ToolExecutionStatus};
    match request {
        StoreRequest::AppendMessage { .. } => "append_message",
        StoreRequest::SaveTool { tool, .. } => match tool.status {
            ToolExecutionStatus::Pending => "tool_intent",
            ToolExecutionStatus::Running => "tool_running",
            _ => "tool_outcome",
        },
        StoreRequest::AppendToolResult { .. } => "tool_result",
        StoreRequest::Complete { .. } => "terminal",
        StoreRequest::WorkspaceCheckpoint { .. } => "workspace_checkpoint",
        StoreRequest::WorkspaceIntegration => "workspace_integration",
        StoreRequest::WorkspaceFinished { .. } => "workspace_finished",
        _ => "other",
    }
}
