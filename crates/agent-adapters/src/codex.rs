//! Codex adapter backed by `codex app-server` over stdio JSONL.

use std::{collections::VecDeque, ffi::OsString, path::PathBuf, sync::Arc};

use async_trait::async_trait;
use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt, BufReader},
    process::{Child, Command},
    sync::mpsc,
};
use tokio_stream::wrappers::ReceiverStream;

use crate::{
    AdapterError, AdapterErrorKind, AgentAdapter, AgentCapabilities, AgentEvent, AgentRunRequest,
    AgentRunStatus, AgentStream, AgentUsage, ApprovalDecision, ApprovalHandler, ApprovalKind,
    ApprovalRequest, DenyAllApprovals,
};

#[derive(Clone)]
pub struct CodexAppServerConfig {
    pub codex_binary: PathBuf,
    pub extra_args: Vec<OsString>,
    pub client_name: String,
    pub client_title: String,
    pub client_version: String,
    pub event_buffer: usize,
    pub approval_handler: Arc<dyn ApprovalHandler>,
}

impl std::fmt::Debug for CodexAppServerConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CodexAppServerConfig")
            .field("codex_binary", &self.codex_binary)
            .field("extra_args", &self.extra_args)
            .field("client_name", &self.client_name)
            .field("client_title", &self.client_title)
            .field("client_version", &self.client_version)
            .field("event_buffer", &self.event_buffer)
            .field("approval_handler", &"<handler>")
            .finish()
    }
}

impl Default for CodexAppServerConfig {
    fn default() -> Self {
        Self {
            codex_binary: PathBuf::from("codex"),
            extra_args: Vec::new(),
            client_name: "local_multi_agent_manager".to_owned(),
            client_title: "Local Multi-Agent Manager".to_owned(),
            client_version: env!("CARGO_PKG_VERSION").to_owned(),
            event_buffer: 128,
            approval_handler: Arc::new(DenyAllApprovals),
        }
    }
}

#[derive(Debug, Clone)]
pub struct CodexAppServerAdapter {
    config: CodexAppServerConfig,
}

impl CodexAppServerAdapter {
    /// Builds an adapter from a validated app-server configuration.
    ///
    /// # Errors
    ///
    /// Returns [`AdapterError`] when the event buffer is empty or required
    /// client identity fields are blank.
    pub fn new(config: CodexAppServerConfig) -> Result<Self, AdapterError> {
        if config.event_buffer == 0 {
            return Err(AdapterError::new(
                AdapterErrorKind::InvalidConfiguration,
                "event_buffer must be greater than zero",
                false,
            ));
        }
        if config.client_name.trim().is_empty() || config.client_version.trim().is_empty() {
            return Err(AdapterError::new(
                AdapterErrorKind::InvalidConfiguration,
                "Codex client name and version must not be empty",
                false,
            ));
        }
        Ok(Self { config })
    }

    fn spawn_process(&self, cwd: &std::path::Path) -> Result<Child, AdapterError> {
        let mut command = Command::new(&self.config.codex_binary);
        command
            .arg("app-server")
            .arg("--listen")
            .arg("stdio://")
            .args(&self.config.extra_args)
            .current_dir(cwd)
            .kill_on_drop(true)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        command.spawn().map_err(|error| {
            AdapterError::new(
                AdapterErrorKind::ProcessSpawn,
                format!("failed to spawn Codex app-server: {error}"),
                false,
            )
        })
    }
}

#[async_trait]
impl AgentAdapter for CodexAppServerAdapter {
    fn driver(&self) -> &'static str {
        "codex_app_server"
    }

    fn capabilities(&self) -> AgentCapabilities {
        AgentCapabilities {
            streaming: true,
            thread_resume: true,
            approvals: true,
            command_execution: true,
            file_changes: true,
            usage: true,
        }
    }

    async fn run(&self, request: AgentRunRequest) -> Result<AgentStream, AdapterError> {
        if request.prompt.trim().is_empty() {
            return Err(AdapterError::new(
                AdapterErrorKind::InvalidConfiguration,
                "Codex prompt must not be empty",
                false,
            ));
        }
        if !request.cwd.is_absolute() {
            return Err(AdapterError::new(
                AdapterErrorKind::InvalidConfiguration,
                "Codex cwd must be absolute",
                false,
            ));
        }

        let mut child = self.spawn_process(&request.cwd)?;
        let stdout = child.stdout.take().ok_or_else(|| {
            AdapterError::new(
                AdapterErrorKind::ProcessSpawn,
                "Codex stdout pipe is unavailable",
                false,
            )
        })?;
        let stdin = child.stdin.take().ok_or_else(|| {
            AdapterError::new(
                AdapterErrorKind::ProcessSpawn,
                "Codex stdin pipe is unavailable",
                false,
            )
        })?;
        let stderr = child.stderr.take();
        let (sender, receiver) = mpsc::channel(self.config.event_buffer);
        let client_info = ClientInfo {
            name: self.config.client_name.clone(),
            title: self.config.client_title.clone(),
            version: self.config.client_version.clone(),
        };
        let approvals = Arc::clone(&self.config.approval_handler);
        tokio::spawn(async move {
            let stderr_task = stderr.map(|stderr| {
                tokio::spawn(async move {
                    // Drain stderr to prevent child backpressure. Never forward it automatically:
                    // diagnostics may contain workspace content.
                    let mut lines = BufReader::new(stderr).lines();
                    while let Ok(Some(_)) = lines.next_line().await {}
                })
            });
            let result =
                drive_protocol(stdout, stdin, request, client_info, approvals, &sender).await;
            if let Err(error) = result {
                let _ = sender.send(Err(error)).await;
            }
            let _ = child.start_kill();
            let _ = child.wait().await;
            if let Some(task) = stderr_task {
                task.abort();
            }
        });
        Ok(Box::pin(ReceiverStream::new(receiver)))
    }
}

#[derive(Debug, Clone)]
#[doc(hidden)]
pub struct ClientInfo {
    pub name: String,
    pub title: String,
    pub version: String,
}

#[doc(hidden)]
#[allow(clippy::too_many_lines)]
pub async fn drive_protocol<R, W>(
    reader: R,
    mut writer: W,
    request: AgentRunRequest,
    client: ClientInfo,
    approvals: Arc<dyn ApprovalHandler>,
    sender: &mpsc::Sender<Result<AgentEvent, AdapterError>>,
) -> Result<(), AdapterError>
where
    R: tokio::io::AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut lines = BufReader::new(reader).lines();

    write_message(
        &mut writer,
        &json!({
            "method": "initialize",
            "id": 0,
            "params": {
                "clientInfo": {
                    "name": client.name,
                    "title": client.title,
                    "version": client.version,
                },
                "capabilities": {"experimentalApi": false}
            }
        }),
    )
    .await?;
    let _ = wait_for_response(&mut lines, 0).await?;
    write_message(&mut writer, &json!({"method": "initialized", "params": {}})).await?;

    let thread_method;
    let thread_params;
    if let Some(thread_id) = &request.resume_thread_id {
        thread_method = "thread/resume";
        thread_params = json!({"threadId": thread_id});
    } else {
        thread_method = "thread/start";
        thread_params = json!({
            "model": request.model,
            "cwd": request.cwd,
            "sandbox": request.sandbox.as_wire_value(),
            "approvalPolicy": request.approval_policy.as_wire_value(),
            "ephemeral": false,
        });
    }
    write_message(
        &mut writer,
        &json!({"method": thread_method, "id": 1, "params": thread_params}),
    )
    .await?;
    let (thread_result, _) = wait_for_response(&mut lines, 1).await?;
    let thread_id = thread_result
        .pointer("/thread/id")
        .and_then(Value::as_str)
        .or(request.resume_thread_id.as_deref())
        .ok_or_else(|| AdapterError::protocol("Codex thread response has no thread id"))?
        .to_owned();
    send_event(
        sender,
        AgentEvent::ThreadStarted {
            thread_id: thread_id.clone(),
        },
    )
    .await?;

    write_message(
        &mut writer,
        &json!({
            "method": "turn/start",
            "id": 2,
            "params": {
                "threadId": thread_id,
                "input": [{"type": "text", "text": request.prompt}],
                "clientUserMessageId": request.request_id,
            }
        }),
    )
    .await?;
    let (turn_result, deferred) = wait_for_response(&mut lines, 2).await?;
    let turn_id = turn_result
        .pointer("/turn/id")
        .and_then(Value::as_str)
        .ok_or_else(|| AdapterError::protocol("Codex turn response has no turn id"))?
        .to_owned();
    send_event(
        sender,
        AgentEvent::TurnStarted {
            turn_id: turn_id.clone(),
        },
    )
    .await?;

    let mut deferred = VecDeque::from(deferred);
    loop {
        let message = if let Some(message) = deferred.pop_front() {
            message
        } else {
            tokio::select! {
                () = request.cancellation.cancelled() => {
                    write_message(
                        &mut writer,
                        &json!({"method": "turn/interrupt", "id": 3, "params": {"threadId": thread_id, "turnId": turn_id}}),
                    ).await?;
                    return Err(AdapterError::cancelled());
                }
                message = read_message(&mut lines) => message?,
            }
        };
        if handle_message(&message, &mut writer, &turn_id, approvals.as_ref(), sender).await? {
            return Ok(());
        }
    }
}

async fn wait_for_response<R>(
    lines: &mut tokio::io::Lines<R>,
    expected_id: i64,
) -> Result<(Value, Vec<Value>), AdapterError>
where
    R: AsyncBufRead + Unpin,
{
    let mut deferred = Vec::new();
    loop {
        let message = read_message(lines).await?;
        if message.get("id").and_then(Value::as_i64) == Some(expected_id) {
            if let Some(error) = message.get("error") {
                return Err(classify_rpc_error(error));
            }
            return Ok((
                message.get("result").cloned().unwrap_or(Value::Null),
                deferred,
            ));
        }
        deferred.push(message);
    }
}

async fn read_message<R>(lines: &mut tokio::io::Lines<R>) -> Result<Value, AdapterError>
where
    R: AsyncBufRead + Unpin,
{
    let line = lines
        .next_line()
        .await
        .map_err(|error| {
            AdapterError::new(
                AdapterErrorKind::Protocol,
                format!("failed reading Codex JSONL: {error}"),
                true,
            )
        })?
        .ok_or_else(|| {
            AdapterError::new(
                AdapterErrorKind::ProcessExited,
                "Codex app-server closed stdout before turn completion",
                true,
            )
        })?;
    serde_json::from_str(&line)
        .map_err(|error| AdapterError::protocol(format!("invalid Codex JSONL: {error}")))
}

async fn write_message<W>(writer: &mut W, message: &Value) -> Result<(), AdapterError>
where
    W: AsyncWrite + Unpin,
{
    let mut bytes = serde_json::to_vec(message).map_err(|error| {
        AdapterError::protocol(format!("failed encoding Codex request: {error}"))
    })?;
    bytes.push(b'\n');
    writer.write_all(&bytes).await.map_err(|error| {
        AdapterError::new(
            AdapterErrorKind::ProcessExited,
            format!("failed writing Codex JSONL: {error}"),
            true,
        )
    })?;
    writer.flush().await.map_err(|error| {
        AdapterError::new(
            AdapterErrorKind::ProcessExited,
            format!("failed flushing Codex JSONL: {error}"),
            true,
        )
    })
}

#[allow(clippy::too_many_lines)]
async fn handle_message<W>(
    message: &Value,
    writer: &mut W,
    turn_id: &str,
    approvals: &dyn ApprovalHandler,
    sender: &mpsc::Sender<Result<AgentEvent, AdapterError>>,
) -> Result<bool, AdapterError>
where
    W: AsyncWrite + Unpin,
{
    let Some(method) = message.get("method").and_then(Value::as_str) else {
        return Ok(false);
    };
    let params = message.get("params").cloned().unwrap_or_else(|| json!({}));
    if let Some(request_id) = message.get("id") {
        let request = ApprovalRequest {
            request_id: request_id.clone(),
            method: method.to_owned(),
            kind: approval_kind(method),
            params,
        };
        send_event(
            sender,
            AgentEvent::ApprovalRequested {
                request: request.clone(),
            },
        )
        .await?;
        let decision = approvals.decide(&request).await;
        let result = approval_response(method, decision)?;
        write_message(writer, &json!({"id": request_id, "result": result})).await?;
        return Ok(false);
    }

    match method {
        "item/agentMessage/delta" => {
            send_event(
                sender,
                AgentEvent::MessageDelta {
                    item_id: required_string(&params, "/itemId")?,
                    delta: required_string(&params, "/delta")?,
                },
            )
            .await?;
        }
        "item/started" => {
            send_event(
                sender,
                AgentEvent::ItemStarted {
                    item: params.get("item").cloned().unwrap_or(Value::Null),
                },
            )
            .await?;
        }
        "item/completed" => {
            send_event(
                sender,
                AgentEvent::ItemCompleted {
                    item: params.get("item").cloned().unwrap_or(Value::Null),
                },
            )
            .await?;
        }
        "thread/tokenUsage/updated" => {
            let usage = params.pointer("/tokenUsage/last").ok_or_else(|| {
                AdapterError::protocol("Codex usage notification has no last usage")
            })?;
            send_event(
                sender,
                AgentEvent::Usage {
                    usage: parse_usage(usage),
                },
            )
            .await?;
        }
        "error" => {
            let error = params.pointer("/error").unwrap_or(&Value::Null);
            let code = error.get("codexErrorInfo").map(compact_code);
            send_event(
                sender,
                AgentEvent::AdapterWarning {
                    message: error
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or("Codex turn error")
                        .to_owned(),
                    retrying: params
                        .get("willRetry")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                    code,
                },
            )
            .await?;
        }
        "turn/completed" => {
            let completed_turn_id = params
                .pointer("/turn/id")
                .and_then(Value::as_str)
                .unwrap_or(turn_id)
                .to_owned();
            let status = match params.pointer("/turn/status").and_then(Value::as_str) {
                Some("completed") => AgentRunStatus::Completed,
                Some("interrupted") => AgentRunStatus::Interrupted,
                Some("failed") => AgentRunStatus::Failed,
                Some("inProgress") => AgentRunStatus::InProgress,
                _ => AgentRunStatus::Unknown,
            };
            let error = params
                .pointer("/turn/error/message")
                .and_then(Value::as_str)
                .map(str::to_owned);
            send_event(
                sender,
                AgentEvent::Completed {
                    turn_id: completed_turn_id,
                    status,
                    error,
                },
            )
            .await?;
            return Ok(true);
        }
        _ => {
            send_event(
                sender,
                AgentEvent::RawNotification {
                    method: method.to_owned(),
                    params,
                },
            )
            .await?;
        }
    }
    Ok(false)
}

fn approval_kind(method: &str) -> ApprovalKind {
    match method {
        "item/commandExecution/requestApproval" => ApprovalKind::CommandExecution,
        "item/fileChange/requestApproval" => ApprovalKind::FileChange,
        "item/permissions/requestApproval" => ApprovalKind::Permissions,
        "execCommandApproval" => ApprovalKind::LegacyCommand,
        "applyPatchApproval" => ApprovalKind::LegacyPatch,
        _ => ApprovalKind::Unsupported,
    }
}

fn approval_response(method: &str, decision: ApprovalDecision) -> Result<Value, AdapterError> {
    if let ApprovalDecision::Raw(value) = decision {
        return Ok(value);
    }
    match method {
        "item/commandExecution/requestApproval" | "item/fileChange/requestApproval" => {
            let decision = match decision {
                ApprovalDecision::Accept => "accept",
                ApprovalDecision::AcceptForSession => "acceptForSession",
                ApprovalDecision::Decline => "decline",
                ApprovalDecision::Cancel => "cancel",
                ApprovalDecision::Raw(_) => unreachable!(),
            };
            Ok(json!({"decision": decision}))
        }
        "execCommandApproval" | "applyPatchApproval" => {
            let decision = match decision {
                ApprovalDecision::Accept => "approved",
                ApprovalDecision::AcceptForSession => "approved_for_session",
                ApprovalDecision::Decline | ApprovalDecision::Cancel => "abort",
                ApprovalDecision::Raw(_) => unreachable!(),
            };
            Ok(json!({"decision": decision}))
        }
        "item/permissions/requestApproval" => match decision {
            ApprovalDecision::Accept | ApprovalDecision::AcceptForSession => {
                Err(AdapterError::new(
                    AdapterErrorKind::Protocol,
                    "permission approvals require ApprovalDecision::Raw with an explicit permission profile",
                    false,
                ))
            }
            ApprovalDecision::Decline | ApprovalDecision::Cancel => {
                Ok(json!({"permissions": {}, "scope": "turn"}))
            }
            ApprovalDecision::Raw(_) => unreachable!(),
        },
        _ => Err(AdapterError::new(
            AdapterErrorKind::Protocol,
            format!("unsupported Codex server request method: {method}"),
            false,
        )),
    }
}

fn parse_usage(value: &Value) -> AgentUsage {
    AgentUsage {
        input_tokens: unsigned(value, "inputTokens"),
        cached_input_tokens: unsigned(value, "cachedInputTokens"),
        output_tokens: unsigned(value, "outputTokens"),
        reasoning_output_tokens: unsigned(value, "reasoningOutputTokens"),
        total_tokens: unsigned(value, "totalTokens"),
    }
}

fn unsigned(value: &Value, key: &str) -> u64 {
    value.get(key).and_then(Value::as_u64).unwrap_or(0)
}

fn required_string(value: &Value, pointer: &str) -> Result<String, AdapterError> {
    value
        .pointer(pointer)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| AdapterError::protocol(format!("Codex notification missing {pointer}")))
}

fn compact_code(value: &Value) -> String {
    value
        .as_str()
        .map_or_else(|| value.to_string(), str::to_owned)
}

fn classify_rpc_error(value: &Value) -> AdapterError {
    let code = value.get("code").map(compact_code);
    let message = value
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("Codex JSON-RPC request failed")
        .to_owned();
    let overloaded = value.get("code").and_then(Value::as_i64) == Some(-32001);
    AdapterError {
        kind: if overloaded {
            AdapterErrorKind::Unavailable
        } else {
            AdapterErrorKind::Protocol
        },
        message,
        retryable: overloaded,
        code,
    }
}

async fn send_event(
    sender: &mpsc::Sender<Result<AgentEvent, AdapterError>>,
    event: AgentEvent,
) -> Result<(), AdapterError> {
    sender.send(Ok(event)).await.map_err(|_| {
        AdapterError::new(
            AdapterErrorKind::Cancelled,
            "agent event receiver was dropped",
            false,
        )
    })
}
