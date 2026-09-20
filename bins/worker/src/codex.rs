//! The only production owner of Codex app-server processes.
use std::{path::PathBuf, sync::Arc};

use ait_agent_adapters::codex::{
    CodexAppServerAdapter, CodexAppServerConfig, CodexSessionTitleGenerator,
};
use ait_contracts::worker::{
    Bootstrap, Executor, Payload, ProtocolError, StoreRequest, StoreResponse,
    codex::{Action, CHUNK_BYTES, MAX_RESULT_BYTES, Operation},
};
use ait_domain::{DomainError, ErrorCode, RunPermissionProfile};
use ait_ipc::{connection::Connection, mapping::Wire, rpc::RemoteStore};
use ait_ports::{
    CodexHistorySource, CodexThreadConnection, CodexThreadInvocation, CodexThreadWriter,
    HostProviderModelCatalog, SessionTitleGenerator, SessionTitleRequest, WorkspaceApproval,
    WorkspaceApprovalDecision, WorkspaceApprovalRequest, WorkspaceProgressEvent,
    WorkspaceProgressReporter,
};
use async_trait::async_trait;
use serde::Serialize;
use tokio_util::sync::CancellationToken;

struct Ports {
    rpc: RemoteStore,
    progress: tokio::sync::Mutex<ProgressBuffer>,
}
struct ProgressBuffer {
    pending: Option<(String, String)>,
    last_flush: std::time::Instant,
}
impl ProgressBuffer {
    async fn flush(&mut self, ports: &Ports) {
        if let Some((id, delta)) = self.pending.take() {
            ports
                .send_progress(WorkspaceProgressEvent::TextDelta { id, delta })
                .await;
            self.last_flush = std::time::Instant::now();
        }
    }
}

fn failed() -> DomainError {
    DomainError::invariant(
        ErrorCode::CodexInputOutcomeUnknown,
        "native worker port unavailable",
    )
}

impl Ports {
    async fn unit(&self, request: StoreRequest) -> Result<(), DomainError> {
        match self.rpc.call(request).await.map_err(|_| failed())? {
            StoreResponse::Unit => Ok(()),
            _ => Err(failed()),
        }
    }

    async fn result<T: Serialize>(
        &self,
        result: Result<T, DomainError>,
        owned: bool,
    ) -> Result<(), ProtocolError> {
        let result = result.and_then(|value| {
            serde_json::to_value(value)
                .map(|value| (value, owned))
                .map_err(|_| failed())
        });
        let bytes = serde_json::to_vec(&result).map_err(|_| ProtocolError::InvalidFrame)?;
        if bytes.len() > MAX_RESULT_BYTES {
            return Err(ProtocolError::ResourceLimit);
        }
        for (index, chunk) in bytes.chunks(CHUNK_BYTES).enumerate() {
            self.unit(StoreRequest::CodexChunk {
                offset: index * CHUNK_BYTES,
                total: bytes.len(),
                bytes: chunk.to_vec(),
            })
            .await
            .map_err(|_| ProtocolError::Io)?;
        }
        Ok(())
    }
}

impl Ports {
    async fn send_progress(&self, event: WorkspaceProgressEvent) {
        if let Ok(mut value) = serde_json::to_value(bounded_progress(event).to_wire()) {
            crate::privacy::redact_display(&mut value);
            if let Ok(event) = serde_json::from_value(value) {
                let _ = self
                    .unit(StoreRequest::WorkspaceProgress {
                        event: Box::new(event),
                    })
                    .await;
            }
        }
    }
}

// A native item can exceed an IPC frame. Only its live preview is shortened;
// the authoritative result still travels through the history chunk protocol.
fn bounded_progress(mut event: WorkspaceProgressEvent) -> WorkspaceProgressEvent {
    if let WorkspaceProgressEvent::MessageStarted { text, .. }
    | WorkspaceProgressEvent::MessageCompleted { text, .. } = &mut event
        && text.len() > 65_536
    {
        text.truncate(text.floor_char_boundary(65_536));
        text.push_str("\n[preview truncated]");
    }
    event
}

#[async_trait]
impl WorkspaceProgressReporter for Ports {
    async fn report(&self, event: WorkspaceProgressEvent) {
        let mut buffer = self.progress.lock().await;
        if let WorkspaceProgressEvent::TextDelta { id, delta } = event {
            if buffer
                .pending
                .as_ref()
                .is_some_and(|(previous, _)| previous != &id)
            {
                buffer.flush(self).await;
            }
            let mut remaining = delta.as_str();
            while !remaining.is_empty() {
                let (_, text) = buffer
                    .pending
                    .get_or_insert_with(|| (id.clone(), String::new()));
                let end = remaining.floor_char_boundary(4096 - text.len());
                text.push_str(&remaining[..end]);
                remaining = &remaining[end..];
                if !remaining.is_empty()
                    || text.len() >= 4096
                    || buffer.last_flush.elapsed() >= std::time::Duration::from_millis(40)
                {
                    buffer.flush(self).await;
                }
            }
        } else {
            buffer.flush(self).await;
            self.send_progress(event).await;
        }
    }
}

#[async_trait]
impl WorkspaceApproval for Ports {
    async fn decide(
        &self,
        request: WorkspaceApprovalRequest,
    ) -> Result<WorkspaceApprovalDecision, DomainError> {
        match self
            .rpc
            .call(StoreRequest::WorkspaceApproval {
                request: Box::new(request.to_wire()),
                expire: false,
            })
            .await
            .map_err(|_| failed())?
        {
            StoreResponse::WorkspaceApproval { decision } => {
                WorkspaceApprovalDecision::from_wire(decision).map_err(|_| failed())
            }
            _ => Err(failed()),
        }
    }

    async fn expire(&self, request: &WorkspaceApprovalRequest) -> Result<(), DomainError> {
        self.unit(StoreRequest::WorkspaceApproval {
            request: Box::new(request.to_wire()),
            expire: true,
        })
        .await
    }
}

async fn writer_loop(
    mut connection: Box<dyn CodexThreadConnection>,
    ports: &Arc<Ports>,
) -> Result<(), ProtocolError> {
    let result = async {
        ports
            .result(
                Ok(connection.prepared()),
                connection.prepared().history.writer_confirmed,
            )
            .await?;
        let mut sent = false;
        loop {
            let StoreResponse::CodexAction { action } = ports
                .rpc
                .call(StoreRequest::CodexNext)
                .await
                .map_err(|_| ProtocolError::Io)?
            else {
                return Err(ProtocolError::InvalidFrame);
            };
            let result = match action {
                Action::Start if !sent => {
                    sent = true;
                    connection.start(ports.clone()).await
                }
                Action::Start => return Err(ProtocolError::InvalidTransition),
                Action::Read => connection.read().await,
                Action::Close => return Ok(()),
            };
            let owned = result
                .as_ref()
                .is_ok_and(|history| history.writer_confirmed);
            ports.progress.lock().await.flush(ports).await;
            ports.result(result, owned).await?;
        }
    }
    .await;
    connection.close().await;
    result
}

async fn run(
    bootstrap: Bootstrap,
    ports: Arc<Ports>,
    cancellation: CancellationToken,
) -> Result<(), ProtocolError> {
    let Executor::Codex { binary, operation } = bootstrap.executor else {
        return Err(ProtocolError::InvalidFrame);
    };
    let adapter = Arc::new(
        CodexAppServerAdapter::new(CodexAppServerConfig {
            codex_binary: PathBuf::from(binary),
            execution_limits: Some(ait_agent_adapters::codex::CodexExecutionLimits {
                max_steps: bootstrap.limits.max_steps,
                max_tokens: bootstrap.limits.max_tokens,
                max_output_bytes: bootstrap
                    .limits
                    .max_codex_output_bytes
                    .unwrap_or(bootstrap.limits.max_output_bytes)
                    as usize,
            }),
            ..Default::default()
        })
        .map_err(|_| ProtocolError::InvalidFrame)?,
    );
    match *operation {
        Operation::Open {
            request_id,
            thread_id,
            prompt,
            model,
            reasoning_effort,
            developer_instructions,
        } => {
            if bootstrap.limits.max_cost_micros.is_some() {
                return Err(ProtocolError::ResourceLimit);
            }
            let result = adapter
                .open(CodexThreadInvocation {
                    project_execution: None,
                    request_id,
                    thread_id,
                    developer_instructions,
                    prompt,
                    cwd: PathBuf::from(bootstrap.workdir),
                    model,
                    reasoning_effort,
                    permission_profile: RunPermissionProfile::from_wire(bootstrap.permission)?,
                    approvals: ports.clone(),
                    cancellation,
                })
                .await;
            match result {
                Ok(connection) => writer_loop(connection, &ports).await?,
                Err(error) => ports.result::<()>(Err(error), false).await?,
            }
        }
        Operation::List { source_kinds } => {
            let kinds: Vec<ait_ports::CodexThreadSourceKind> = serde_json::from_value(
                serde_json::to_value(source_kinds).map_err(|_| ProtocolError::InvalidFrame)?,
            )
            .map_err(|_| ProtocolError::InvalidFrame)?;
            ports
                .result(adapter.list_threads(&kinds).await, false)
                .await?;
        }
        Operation::Read { thread_id } => {
            ports
                .result(adapter.read_thread(&thread_id).await, false)
                .await?;
        }
        Operation::Models { provider } => {
            ports
                .result(adapter.discover_models(&provider).await, false)
                .await?;
        }
        Operation::Title {
            request_id,
            user_prompt,
            config,
            provider,
        } => {
            if bootstrap.limits.max_cost_micros.is_some() {
                return Err(ProtocolError::ResourceLimit);
            }
            let generator = CodexSessionTitleGenerator::new(adapter);
            let result = generator
                .generate(SessionTitleRequest {
                    request_id,
                    user_prompt,
                    config,
                    provider,
                    credential_ref: None,
                    cwd: PathBuf::from(bootstrap.workdir),
                    cancellation,
                })
                .await;
            ports.result(result, false).await?;
        }
    }
    ports
        .unit(StoreRequest::CodexClosed)
        .await
        .map_err(|_| ProtocolError::Io)
}

pub(crate) async fn execute(
    bootstrap: Bootstrap,
    mut pipe: Connection,
) -> Result<(), ProtocolError> {
    let (rpc, mut calls) = RemoteStore::channel();
    let ports = Arc::new(Ports {
        rpc,
        progress: tokio::sync::Mutex::new(ProgressBuffer {
            pending: None,
            last_flush: std::time::Instant::now(),
        }),
    });
    let cancellation = CancellationToken::new();
    let mut task = tokio_util::task::AbortOnDropHandle::new(tokio::spawn(run(
        bootstrap.clone(),
        ports,
        cancellation.clone(),
    )));
    let outcome = {
        let io = crate::stdio::drive_io(&mut pipe, &mut calls, &bootstrap, cancellation.clone());
        tokio::pin!(io);
        tokio::select! {
            result = &mut task => result.unwrap_or(Err(ProtocolError::WorkerExited)),
            result = &mut io => { cancellation.cancel(); task.abort(); let _ = (&mut task).await; result }
        }
    };
    cancellation.cancel();
    if outcome.is_ok() {
        let _ = pipe.send(Payload::ExitReport).await;
    }
    pipe.close().await;
    outcome
}
