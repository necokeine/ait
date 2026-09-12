//! Native harness composition, retaining the existing Git settlement protocol.
use ait_contracts::worker::{
    Bootstrap, Executor, Payload, ProtocolError, StoreRequest, StoreResponse,
};
use ait_domain::{DomainError, ErrorCode, RunPermissionProfile};
use ait_ipc::{connection::Connection, mapping::Wire, rpc::RemoteStore};
use ait_ports::{
    WorkspaceAgent, WorkspaceAgentInvocation, WorkspaceAgentResponse, WorkspaceApproval,
    WorkspaceApprovalDecision, WorkspaceApprovalRequest, WorkspaceIntegrationGate,
    WorkspaceProgressEvent, WorkspaceProgressReporter, WorkspaceResultSink,
};
use async_trait::async_trait;
use std::{path::PathBuf, sync::Arc};
use tokio_util::sync::CancellationToken;

struct BoundedCodex {
    inner: ait_agent_adapters::codex::CodexAppServerAdapter,
    limits: ait_contracts::worker::Limits,
}
#[async_trait]
impl ait_agent_adapters::AgentAdapter for BoundedCodex {
    fn driver(&self) -> &'static str {
        self.inner.driver()
    }
    fn capabilities(&self) -> ait_agent_adapters::AgentCapabilities {
        self.inner.capabilities()
    }
    async fn run(
        &self,
        request: ait_agent_adapters::AgentRunRequest,
    ) -> Result<ait_agent_adapters::AgentStream, ait_agent_adapters::AdapterError> {
        use ait_agent_adapters::AgentEvent;
        use futures_util::StreamExt;
        if self.limits.max_cost_micros.is_some() {
            return Err(ait_agent_adapters::AdapterError::new(
                ait_agent_adapters::AdapterErrorKind::InvalidConfiguration,
                "configured cost ceiling requires verified Provider pricing; invocation denied",
                false,
            ));
        }
        let cancellation = request.cancellation.clone();
        let limits = self.limits.clone();
        let stream = self.inner.run(request).await?;
        let mut items = std::collections::HashSet::new();
        let mut output_bytes = 0_usize;
        Ok(Box::pin(stream.map(move |mut event| {
            let mut exceeded = false;
            match &mut event {
                Ok(AgentEvent::ItemStarted { item } | AgentEvent::ItemCompleted { item }) => {
                    if let Some(id) = item.get("id").and_then(serde_json::Value::as_str) {
                        items.insert(id.to_owned());
                    }
                    exceeded = u64::try_from(items.len()).unwrap_or(u64::MAX) > limits.max_steps
                        || serde_json::to_vec(item)
                            .map_or(true, |bytes| bytes.len() > limits.max_output_bytes as usize);
                    crate::privacy::redact_display(item);
                }
                Ok(AgentEvent::MessageDelta { delta, .. }) => {
                    output_bytes = output_bytes.saturating_add(delta.len());
                    exceeded = output_bytes > limits.max_output_bytes as usize;
                }
                Ok(AgentEvent::Usage { usage }) => {
                    exceeded = usage.total_tokens.max(
                        usage
                            .input_tokens
                            .saturating_add(usage.output_tokens)
                            .saturating_add(usage.cached_input_tokens),
                    ) > limits.max_tokens;
                }
                Ok(AgentEvent::AdapterWarning { message, code, .. }) => {
                    *message = "native provider reported a warning".into();
                    *code = None;
                }
                Ok(AgentEvent::Completed {
                    error: Some(error), ..
                }) => *error = "native provider turn failed".into(),
                Err(error) => error.message = "native provider request failed".into(),
                _ => {}
            }
            if exceeded {
                cancellation.cancel();
                Err(ait_agent_adapters::AdapterError::protocol(
                    "native Run resource limit exceeded",
                ))
            } else {
                event
            }
        })))
    }
}

#[derive(Clone)]
struct Ports {
    rpc: RemoteStore,
    progress: Arc<tokio::sync::Mutex<ProgressBuffer>>,
}
struct ProgressBuffer {
    pending: Option<(String, String)>,
    last_flush: std::time::Instant,
}
impl ProgressBuffer {
    async fn flush(&mut self, ports: &Ports) {
        if let Some((id, delta)) = self.pending.take() {
            let _ = ports
                .unit(StoreRequest::WorkspaceProgress {
                    event: Box::new(WorkspaceProgressEvent::TextDelta { id, delta }.to_wire()),
                })
                .await;
            self.last_flush = std::time::Instant::now();
        }
    }
}
impl std::fmt::Debug for Ports {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("worker daemon ports")
    }
}
fn failed() -> DomainError {
    DomainError::invariant(
        ErrorCode::RunRecoveryFailed,
        "workspace daemon port unavailable",
    )
}
impl Ports {
    async fn unit(&self, request: StoreRequest) -> Result<(), DomainError> {
        match self.rpc.call(request).await.map_err(|_| failed())? {
            StoreResponse::Unit => Ok(()),
            _ => Err(failed()),
        }
    }
}
#[async_trait]
impl WorkspaceResultSink for Ports {
    async fn checkpoint(&self, result: WorkspaceAgentResponse) -> Result<(), DomainError> {
        self.unit(StoreRequest::WorkspaceCheckpoint {
            result: Box::new(result.to_wire()),
        })
        .await
    }
}
#[async_trait]
impl WorkspaceIntegrationGate for Ports {
    async fn begin_integration(&self) -> Result<(), DomainError> {
        self.unit(StoreRequest::WorkspaceIntegration).await
    }
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
            let (_, text) = buffer.pending.get_or_insert_with(|| (id, String::new()));
            text.push_str(&delta);
            if text.len() >= 4096
                || buffer.last_flush.elapsed() >= std::time::Duration::from_millis(40)
            {
                buffer.flush(self).await;
            }
            return;
        }
        buffer.flush(self).await;
        let _ = self
            .unit(StoreRequest::WorkspaceProgress {
                event: Box::new(event.to_wire()),
            })
            .await;
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
pub(crate) async fn execute(
    bootstrap: Bootstrap,
    mut pipe: Connection,
) -> Result<(), ProtocolError> {
    let Executor::Workspace { invocation } = bootstrap.executor.clone() else {
        return Err(ProtocolError::InvalidFrame);
    };
    let (store, mut calls) = RemoteStore::channel();
    let ports = Arc::new(Ports {
        rpc: store,
        progress: Arc::new(tokio::sync::Mutex::new(ProgressBuffer {
            pending: None,
            last_flush: std::time::Instant::now(),
        })),
    });
    let cancellation = CancellationToken::new();
    let run_cancel = cancellation.clone();
    let run_bootstrap = bootstrap.clone();
    let mut run = tokio_util::task::AbortOnDropHandle::new(tokio::spawn(async move {
        let bootstrap = run_bootstrap;
        let cancellation = run_cancel;
        let adapter = ait_agent_adapters::codex::CodexAppServerAdapter::new(
            ait_agent_adapters::codex::CodexAppServerConfig {
                codex_binary: PathBuf::from(invocation.codex_binary),
                ..Default::default()
            },
        )
        .map_err(|_| ProtocolError::InvalidFrame)?;
        let agent = ait_agent_adapters::codex::CodexWorkspaceAgent::new(Arc::new(BoundedCodex {
            inner: adapter,
            limits: bootstrap.limits.clone(),
        }));
        let request = WorkspaceAgentInvocation {
            request_id: invocation.request_id,
            model: invocation.model,
            reasoning_effort: invocation.reasoning_effort,
            project_instructions: invocation.project_instructions,
            prompt: invocation.prompt,
            commit_subject: invocation.commit_subject,
            cwd: PathBuf::from(&bootstrap.workdir),
            baseline_commit: invocation.baseline_commit,
            baseline_index_tree: invocation.baseline_index_tree,
            permission_profile: RunPermissionProfile::from_wire(bootstrap.permission.clone())?,
            approvals: ports.clone(),
            cancellation: cancellation.clone(),
            integration_gate: Some(ports.clone()),
        };
        let result = if let Some(result) = invocation.recovery_result {
            agent
                .recover_checkpointed(
                    request,
                    WorkspaceAgentResponse::from_wire(result)?,
                    invocation.baseline_ref,
                )
                .await
        } else {
            agent
                .invoke_with_progress_and_checkpoint(request, ports.clone(), ports.as_ref())
                .await
        }
        .map_err(|_| ProtocolError::InvalidTransition)?;
        ports
            .unit(StoreRequest::WorkspaceFinished {
                result: Box::new(result.to_wire()),
            })
            .await
            .map_err(|_| ProtocolError::InvalidTransition)
    }));
    let outcome = {
        let io = crate::stdio::drive_io(&mut pipe, &mut calls, &bootstrap, cancellation.clone());
        tokio::pin!(io);
        tokio::select! {
            result=&mut run=>result.unwrap_or(Err(ProtocolError::WorkerExited)),
            result=&mut io=>{cancellation.cancel();run.abort();let _=(&mut run).await;result}
        }
    };
    cancellation.cancel();
    if outcome.is_ok() {
        let _ = pipe.send(Payload::ExitReport).await;
    }
    pipe.close().await;
    outcome
}
