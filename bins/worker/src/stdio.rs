//! Worker composition root. Authoritative state is available only through RPC.
use ait_contracts::worker::{Bootstrap, Executor, Hello, MAX_FRAME_BYTES, Payload, ProtocolError};
use ait_domain::{
    AgentConfiguration, DomainError, ErrorCode, RunId, RunPermissionProfile, RunUsage, SubMessage,
};
use ait_ipc::{
    codec::{Reader, Writer},
    connection::Connection,
    mapping::Wire,
    rpc::{Call, RemoteStore},
};
use ait_ports::{AgentInvocation, AgentResponse, RunAgent};
use async_trait::async_trait;
use std::{collections::HashMap, path::Path, sync::Arc, time::Duration};
use tokio::{
    sync::{mpsc, oneshot},
    time::Instant,
};
use tokio_util::sync::CancellationToken;

struct ApiAgent {
    client: ait_agent_adapters::LLMClient,
    config: AgentConfiguration,
    names: Vec<String>,
    requires_verified_cost: bool,
}
#[async_trait]
impl RunAgent for ApiAgent {
    async fn invoke(&self, request: AgentInvocation) -> Result<AgentResponse, DomainError> {
        if self.requires_verified_cost {
            return Err(DomainError::invariant(
                ErrorCode::RunLimitExceeded,
                "configured cost ceiling requires verified Provider pricing; invocation denied",
            ));
        }
        let response = ait_agent_adapters::provider_turn::complete_turn(
            &self.client,
            &self.config,
            request,
            &self.names,
        )
        .await?;
        for part in &response.sub_messages {
            if let SubMessage::ToolUse(tool) = part {
                let unsafe_input = tool.arguments.len() > 16_384
                    || serde_json::from_str(&tool.arguments)
                        .map_or(true, |value| crate::privacy::unsafe_arguments(&value));
                if unsafe_input {
                    return Err(DomainError::invariant(
                        ErrorCode::ToolApprovalRequired,
                        "tool arguments exceed the private input policy",
                    ));
                }
            }
        }
        Ok(response)
    }
}
struct ScriptedAgent {
    replies: Vec<Vec<SubMessage>>,
}
#[async_trait]
impl RunAgent for ScriptedAgent {
    async fn invoke(&self, request: AgentInvocation) -> Result<AgentResponse, DomainError> {
        let index=request.message_path.iter().filter(|entry|matches!(entry,ait_domain::ProjectedMessage::Visible(m) if m.run_id.as_ref()==Some(&request.run_id)&&m.role==ait_domain::MessageRole::Assistant)).count();
        let reply = self.replies.get(index).ok_or_else(|| {
            DomainError::invariant(ErrorCode::ProviderFailed, "scripted response exhausted")
        })?;
        Ok(AgentResponse {
            sub_messages: reply.clone(),
            usage: RunUsage::default(),
        })
    }
}
/// Runs a handshake and a Run, then joins tools and protocol pumps.
/// # Errors
/// Returns stable codes without credentials, input, or SDK diagnostics.
pub async fn serve() -> Result<(), ProtocolError> {
    let mut reader = Reader::new(tokio::io::stdin(), MAX_FRAME_BYTES);
    let mut writer = Writer::new(tokio::io::stdout(), MAX_FRAME_BYTES);
    let bootstrap = tokio::time::timeout(Duration::from_secs(3), async {
        writer.write(None, Payload::Hello(Hello::current())).await?;
        let ack = reader.read().await?;
        if ack.lease.is_some() || !matches!(ack.payload, Payload::HelloAck) {
            return Err(ProtocolError::VersionMismatch);
        }
        let frame = reader.read().await?;
        let Payload::Bootstrap(bootstrap) = frame.payload else {
            return Err(ProtocolError::InvalidFrame);
        };
        if frame.lease.as_ref() != Some(&bootstrap.lease) {
            return Err(ProtocolError::StaleWorkerLease);
        }
        bootstrap.limits.validate()?;
        reader.constrain(bootstrap.limits.max_frame_bytes);
        writer.constrain(bootstrap.limits.max_frame_bytes);
        if bootstrap.permission.sandbox > bootstrap.maximum_sandbox {
            return Err(ProtocolError::ResourceLimit);
        }
        writer
            .write(
                Some(bootstrap.lease.clone()),
                Payload::Ready {
                    pid: std::process::id(),
                },
            )
            .await?;
        Ok(*bootstrap)
    })
    .await
    .map_err(|_| ProtocolError::HandshakeTimeout)??;
    let secret = match &bootstrap.executor {
        Executor::Api { credential, .. } => Some(credential.0.clone()),
        Executor::Scripted { .. } | Executor::Workspace { .. } => None,
    };
    let pipe = Connection::start(reader, writer, bootstrap.lease.clone(), secret);
    if matches!(bootstrap.executor, Executor::Workspace { .. }) {
        return crate::workspace::execute(bootstrap, pipe).await;
    }
    execute(bootstrap, pipe).await
}
async fn execute(bootstrap: Bootstrap, mut pipe: Connection) -> Result<(), ProtocolError> {
    let cancellation = CancellationToken::new();
    let outcome = async {
        let profile = RunPermissionProfile::from_wire(bootstrap.permission.clone())?;
        let factory = ait_sandbox::SandboxToolFactory {
            maximum: ait_domain::SandboxAccess::from_wire(bootstrap.maximum_sandbox)?,
        };
        let tools = factory
            .create_bounded(
                Path::new(&bootstrap.workdir),
                profile,
                bootstrap.limits.max_output_bytes,
                bootstrap.limits.max_tool_concurrency,
            )
            .map_err(|_| ProtocolError::ResourceLimit)?;
        let agent: Arc<dyn RunAgent> = match bootstrap.executor.clone() {
            Executor::Workspace { .. } => return Err(ProtocolError::InvalidFrame),
            Executor::Api {
                provider,
                endpoint,
                model,
                reasoning_effort,
                credential,
            } => {
                let provider = match provider.as_str() {
                    "openai" => ait_agent_adapters::LLMProvider::OpenAI,
                    "deepseek" => ait_agent_adapters::LLMProvider::DeepSeek,
                    _ => return Err(ProtocolError::InvalidFrame),
                };
                let mut config = ait_agent_adapters::LLMClientConfig::new(provider, credential.0);
                config.base_url = endpoint;
                let client = ait_agent_adapters::LLMClient::new(config)
                    .map_err(|_| ProtocolError::InvalidFrame)?;
                Arc::new(ApiAgent {
                    client,
                    config: AgentConfiguration {
                        provider_id: String::new(),
                        model,
                        reasoning_effort,
                    },
                    names: tools.executable_tools(),
                    requires_verified_cost: bootstrap.limits.max_cost_micros.is_some(),
                })
            }
            Executor::Scripted { replies } => Arc::new(ScriptedAgent {
                replies: replies
                    .into_iter()
                    .map(Vec::<SubMessage>::from_wire)
                    .collect::<Result<_, _>>()?,
            }),
        };
        let (store, mut calls) = RemoteStore::channel();
        let store = Arc::new(store);
        let worker = crate::RunWorker::new(
            store.clone(),
            agent,
            tools.clone(),
            store,
        );
        let id = RunId::new(&bootstrap.lease.run_id);
        let run_cancel = cancellation.clone();
        let mut drive = tokio_util::task::AbortOnDropHandle::new(tokio::spawn(async move { worker.execute(&id, run_cancel).await }));
        let result = {
            let io = drive_io(&mut pipe, &mut calls, &bootstrap, cancellation.clone());
            tokio::pin!(io);
            tokio::select! {
                result=&mut drive=>result.map_err(|_|ProtocolError::WorkerExited)?.map(|_|()).map_err(|_|ProtocolError::InvalidTransition),
                result=&mut io=>{cancellation.cancel();drive.abort();let _=(&mut drive).await;result},
            }
        };
        cancellation.cancel();
        let drained = tokio::time::timeout(
            Duration::from_millis(bootstrap.limits.drain_ms),
            tools.cancel_and_drain(),
        )
        .await;
        if drained.is_err() {
            return Err(ProtocolError::WorkerExited);
        }
        result
    }
    .await;
    if outcome.is_ok() {
        let _ = pipe.send(Payload::ExitReport).await;
    }
    pipe.close().await;
    outcome
}
type Pending = HashMap<
    u64,
    (
        String,
        oneshot::Sender<Result<ait_contracts::worker::StoreResponse, ProtocolError>>,
    ),
>;
pub(crate) async fn drive_io(
    pipe: &mut Connection,
    calls: &mut mpsc::Receiver<Call>,
    bootstrap: &Bootstrap,
    cancel: CancellationToken,
) -> Result<(), ProtocolError> {
    let mut pending = Pending::new();
    let mut request_id = 0_u64;
    let mut interval = tokio::time::interval(Duration::from_millis(bootstrap.limits.heartbeat_ms));
    let mut heartbeat = Instant::now();
    loop {
        tokio::select! {
            Some(call)=calls.recv()=>{
                if pending.len()>=16{return Err(ProtocolError::ResourceLimit)}
                request_id=request_id.checked_add(1).ok_or(ProtocolError::ResourceLimit)?;
                let operation_id=uuid::Uuid::new_v4().to_string();
                pipe.send(Payload::Request{request_id,operation_id:operation_id.clone(),request:Box::new(call.request)}).await?;
                pending.insert(request_id,(operation_id,call.reply));
            },
            frame=pipe.receiver.recv()=>{
                let frame=frame.ok_or(ProtocolError::UnexpectedEof)??;
                if frame.lease.as_ref()!=Some(&bootstrap.lease){return Err(ProtocolError::StaleWorkerLease)}
                heartbeat=Instant::now();
                match frame.payload {
                    Payload::Heartbeat=>{},
                    Payload::Cancel=>cancel.cancel(),
                    Payload::Receipt{request_id,operation_id,response}=>{
                        let (expected,reply)=pending.remove(&request_id).ok_or(ProtocolError::CorrelationMismatch)?;
                        if expected!=operation_id{return Err(ProtocolError::CorrelationMismatch)}
                        let _=reply.send(Ok(*response));
                    },
                    Payload::Rejected{request_id,code}=>{
                        let (_,reply)=pending.remove(&request_id).ok_or(ProtocolError::CorrelationMismatch)?;let _=reply.send(Err(code));
                    },
                    _=>return Err(ProtocolError::InvalidFrame),
                }
            },
            _=interval.tick()=>{
                if heartbeat.elapsed()>Duration::from_millis(bootstrap.limits.heartbeat_timeout_ms){return Err(ProtocolError::HeartbeatTimeout)}
                pipe.send(Payload::Heartbeat).await?;
            }
        }
    }
}
