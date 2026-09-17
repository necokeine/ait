use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::Arc,
};

use ait_domain::{
    NativeApprovalFileChange, NativeApprovalFileChangeKind, NativeApprovalTarget,
    NativeNetworkProtocol, ProviderModel,
};
use ait_tools::codex::CodexToolSet;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt, BufReader},
    sync::mpsc,
    task::{AbortHandle, JoinSet},
};

use crate::{
    AdapterError, AdapterErrorKind, AgentEvent, AgentRunRequest, AgentRunStatus, AgentUsage,
    ApprovalDecision, ApprovalHandler, ApprovalKind, ApprovalRequest,
};

#[derive(Debug, Clone)]
#[doc(hidden)]
pub struct ClientInfo {
    pub name: String,
    pub title: String,
    pub version: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CodexModelPage {
    data: Vec<CodexModel>,
    next_cursor: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CodexModel {
    model: String,
    #[serde(default)]
    display_name: String,
    #[serde(default)]
    supported_reasoning_efforts: Vec<CodexReasoningEffort>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CodexReasoningEffort {
    reasoning_effort: String,
}

async fn initialize_protocol<R, W>(
    lines: &mut tokio::io::Lines<R>,
    writer: &mut W,
    client: ClientInfo,
) -> Result<(), AdapterError>
where
    R: AsyncBufRead + Unpin,
    W: AsyncWrite + Unpin,
{
    write_message(
        writer,
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
    let _ = wait_for_response(lines, 0).await?;
    write_message(writer, &json!({"method": "initialized", "params": {}})).await
}

/// Drives the initialized, paginated `model/list` exchange used by host model
/// discovery without starting a Codex thread or turn.
#[doc(hidden)]
pub async fn drive_model_list_protocol<R, W>(
    reader: R,
    mut writer: W,
    client: ClientInfo,
) -> Result<Vec<ProviderModel>, AdapterError>
where
    R: tokio::io::AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut lines = BufReader::new(reader).lines();
    initialize_protocol(&mut lines, &mut writer, client).await?;
    let mut models = Vec::new();
    let mut model_ids = HashSet::new();
    let mut seen_cursors = HashSet::new();
    let mut cursor: Option<String> = None;
    let mut request_id = 1_i64;
    loop {
        let mut params = json!({"limit": 100, "includeHidden": false});
        if let Some(value) = &cursor {
            params["cursor"] = json!(value);
        }
        write_message(
            &mut writer,
            &json!({"method": "model/list", "id": request_id, "params": params}),
        )
        .await?;
        let (result, _) = wait_for_response(&mut lines, request_id).await?;
        let page: CodexModelPage = serde_json::from_value(result).map_err(|error| {
            AdapterError::protocol(format!("invalid Codex model/list response: {error}"))
        })?;
        for model in page.data {
            let id = model.model.trim().to_owned();
            if id.is_empty() || !model_ids.insert(id.clone()) {
                continue;
            }
            let mut efforts = Vec::new();
            let mut seen_efforts = HashSet::new();
            for effort in model.supported_reasoning_efforts {
                let effort = effort.reasoning_effort.trim().to_owned();
                if !effort.is_empty() && seen_efforts.insert(effort.clone()) {
                    efforts.push(effort);
                }
            }
            let name = model.display_name.trim();
            models.push(ProviderModel {
                id: id.clone(),
                name: if name.is_empty() { id } else { name.to_owned() },
                reasoning_efforts: efforts,
            });
        }
        let Some(next) = page.next_cursor.filter(|value| !value.trim().is_empty()) else {
            return Ok(models);
        };
        if !seen_cursors.insert(next.clone()) {
            return Err(AdapterError::protocol(
                "Codex model/list returned a repeated pagination cursor",
            ));
        }
        cursor = Some(next);
        request_id += 1;
    }
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
    initialize_protocol(&mut lines, &mut writer, client).await?;

    // Keep the model-specific base prompt and native tools owned by codex-core.
    // Apply the same Ait/Project developer layer on new and resumed threads.
    let mut thread_params = json!({
        "model": request.model.as_deref(),
        "cwd": request.cwd,
        "sandbox": request.sandbox.as_wire_value(),
        "approvalPolicy": request.approval_policy.as_wire_value(),
        "developerInstructions": CodexToolSet.developer_instructions(request.project_instructions.as_deref()),
    });
    let thread_method;
    if let Some(thread_id) = &request.resume_thread_id {
        thread_method = "thread/resume";
        thread_params["threadId"] = json!(thread_id);
    } else {
        thread_method = "thread/start";
        thread_params["ephemeral"] = json!(request.ephemeral);
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

    let mut turn_params = json!({
        "threadId": thread_id,
        "input": [{"type": "text", "text": request.prompt}],
        "clientUserMessageId": request.request_id,
        "cwd": request.cwd,
        "approvalPolicy": request.approval_policy.as_wire_value(),
        // Explicitly replace native defaults, including user-configured write
        // roots and implicit temporary-directory access, on every new/resumed turn.
        "sandboxPolicy": match request.sandbox {
            crate::SandboxMode::ReadOnly => json!({"type": "readOnly", "networkAccess": false}),
            crate::SandboxMode::WorkspaceWrite => json!({
                "type": "workspaceWrite",
                "writableRoots": [request.cwd],
                "networkAccess": false,
                "excludeTmpdirEnvVar": true,
                "excludeSlashTmp": true,
            }),
            crate::SandboxMode::DangerFullAccess => json!({"type": "dangerFullAccess"}),
        },
    });
    if let Some(effort) = request.reasoning_effort {
        turn_params["effort"] = json!(effort);
    }
    if let Some(output_schema) = request.output_schema {
        turn_params["outputSchema"] = output_schema;
    }
    write_message(
        &mut writer,
        &json!({"method": "turn/start", "id": 2, "params": turn_params}),
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
    let mut approval_tasks = JoinSet::<ApprovalResolution>::new();
    let mut pending_approvals = HashMap::<String, PendingApprovalTask>::new();
    let mut seen_server_requests = HashSet::new();
    let mut answered_server_requests = HashSet::new();
    let mut approval_items = HashMap::<String, Value>::new();
    loop {
        let message = if let Some(message) = deferred.pop_front() {
            Some(message)
        } else {
            // A buffered invalidation must revoke a request before a ready handler can answer it.
            tokio::select! {
                biased;
                () = request.cancellation.cancelled() => {
                    write_message(
                        &mut writer,
                        &json!({"method": "turn/interrupt", "id": 3, "params": {"threadId": thread_id, "turnId": turn_id}}),
                    ).await?;
                    approval_tasks.abort_all();
                    expire_protocol_approvals(&approvals, &mut pending_approvals).await;
                    return Err(AdapterError::cancelled());
                }
                message = read_message(&mut lines) => Some(message?),
                resolution = approval_tasks.join_next(), if !approval_tasks.is_empty() => {
                    let Some(resolution) = resolution else {
                        continue;
                    };
                    match resolution {
                        Ok(resolution) => {
                            let Some(pending) = pending_approvals.remove(&resolution.request_key) else {
                                continue;
                            };
                            if resolution.decision == ApprovalDecision::Cancel {
                                write_message(
                                    &mut writer,
                                    &json!({"method": "turn/interrupt", "id": 3, "params": {"threadId": thread_id, "turnId": turn_id}}),
                                )
                                .await?;
                                approvals.resolved(&pending.request).await;
                                approval_tasks.abort_all();
                                expire_protocol_approvals(&approvals, &mut pending_approvals).await;
                                return Err(AdapterError::cancelled());
                            }
                            match approval_response(&resolution.method, resolution.decision) {
                                Ok(result) => {
                                    write_message(
                                        &mut writer,
                                        &json!({"id": resolution.request_id, "result": result}),
                                    )
                                    .await?;
                                }
                                Err(error) => {
                                    send_event(
                                        sender,
                                        AgentEvent::AdapterWarning {
                                            message: format!(
                                                "Codex server request {} was rejected: {}",
                                                resolution.method, error.message
                                            ),
                                            retrying: false,
                                            code: Some("CODEX_SERVER_REQUEST_INVALID_RESPONSE".into()),
                                        },
                                    )
                                    .await?;
                                    write_rpc_error(
                                        &mut writer,
                                        &resolution.request_id,
                                        -32602,
                                        error.message,
                                    )
                                    .await?;
                                }
                            }
                            answered_server_requests.insert(resolution.request_key);
                        }
                        Err(error) if error.is_cancelled() => {}
                        Err(error) => {
                            return Err(AdapterError::new(
                                AdapterErrorKind::Protocol,
                                format!("Codex approval handler task failed: {error}"),
                                false,
                            ));
                        }
                    }
                    None
                }
            }
        };
        let Some(message) = message else {
            continue;
        };
        let method = message.get("method").and_then(Value::as_str);
        if method == Some("item/started")
            && let Some(item) = message.pointer("/params/item")
            && let Some(item_id) = item.get("id").and_then(Value::as_str)
        {
            approval_items.insert(item_id.to_owned(), item.clone());
        }
        if method == Some("serverRequest/resolved")
            && let Some(request_id) = message.pointer("/params/requestId")
            && let Some(request_key) = server_request_key(request_id)
            && let Some(pending) = pending_approvals.remove(&request_key)
        {
            pending.task.abort();
            approvals.resolved(&pending.request).await;
        }
        if let (Some(method), Some(request_id)) = (method, message.get("id")) {
            let params = message.get("params").cloned().unwrap_or_else(|| json!({}));
            handle_server_request(
                request_id,
                method,
                params,
                &thread_id,
                &turn_id,
                &mut writer,
                Arc::clone(&approvals),
                sender,
                &mut approval_tasks,
                &mut pending_approvals,
                &mut seen_server_requests,
                &mut answered_server_requests,
                &approval_items,
            )
            .await?;
            continue;
        }
        if handle_message(&message, &turn_id, sender).await? {
            return Ok(());
        }
        if method == Some("item/completed")
            && let Some(item_id) = message.pointer("/params/item/id").and_then(Value::as_str)
        {
            approval_items.remove(item_id);
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
async fn handle_message(
    message: &Value,
    turn_id: &str,
    sender: &mpsc::Sender<Result<AgentEvent, AdapterError>>,
) -> Result<bool, AdapterError> {
    let Some(method) = message.get("method").and_then(Value::as_str) else {
        return Ok(false);
    };
    let params = message.get("params").cloned().unwrap_or_else(|| json!({}));
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

#[derive(Debug, Clone)]
struct ApprovalResolution {
    request_id: Value,
    request_key: String,
    method: String,
    decision: ApprovalDecision,
}

struct PendingApprovalTask {
    task: AbortHandle,
    request: ApprovalRequest,
}

async fn expire_protocol_approvals(
    approvals: &Arc<dyn ApprovalHandler>,
    pending_approvals: &mut HashMap<String, PendingApprovalTask>,
) {
    for (_, pending) in pending_approvals.drain() {
        pending.task.abort();
        approvals.resolved(&pending.request).await;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ServerRequestKind {
    Approval(ApprovalKind),
    McpElicitation,
    UserInput,
    DynamicTool,
    AuthTokenRefresh,
    Attestation,
    Unsupported,
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
async fn handle_server_request<W>(
    request_id: &Value,
    method: &str,
    params: Value,
    thread_id: &str,
    turn_id: &str,
    writer: &mut W,
    approvals: Arc<dyn ApprovalHandler>,
    sender: &mpsc::Sender<Result<AgentEvent, AdapterError>>,
    approval_tasks: &mut JoinSet<ApprovalResolution>,
    pending_approvals: &mut HashMap<String, PendingApprovalTask>,
    seen_server_requests: &mut HashSet<String>,
    answered_server_requests: &mut HashSet<String>,
    approval_items: &HashMap<String, Value>,
) -> Result<(), AdapterError>
where
    W: AsyncWrite + Unpin,
{
    let Some(request_key) = server_request_key(request_id) else {
        write_rpc_error(
            writer,
            request_id,
            -32600,
            "Codex server request id must be a string or integer",
        )
        .await?;
        return Ok(());
    };
    if !seen_server_requests.insert(request_key.clone()) {
        send_server_request_warning(
            sender,
            method,
            "used a duplicate request id",
            "CODEX_SERVER_REQUEST_DUPLICATE",
        )
        .await?;
        if !answered_server_requests.contains(&request_key)
            && let Some(pending) = pending_approvals.remove(&request_key)
        {
            pending.task.abort();
            approvals.resolved(&pending.request).await;
            write_rpc_error(
                writer,
                request_id,
                -32600,
                format!("duplicate Codex server request id for method {method}"),
            )
            .await?;
            answered_server_requests.insert(request_key);
        }
        return Ok(());
    }

    let request_kind = server_request_kind(method);
    match request_kind {
        ServerRequestKind::Approval(kind) => {
            if let Err(reason) = validate_approval_correlation(method, &params, thread_id, turn_id)
            {
                send_server_request_warning(
                    sender,
                    method,
                    &reason,
                    "CODEX_APPROVAL_CORRELATION_INVALID",
                )
                .await?;
                write_rpc_error(writer, request_id, -32602, reason).await?;
                answered_server_requests.insert(request_key);
                return Ok(());
            }
            let target = match approval_target(kind, &params, approval_items) {
                Ok(target) => target,
                Err(reason) => {
                    send_server_request_warning(
                        sender,
                        method,
                        &reason,
                        "CODEX_APPROVAL_TARGET_INVALID",
                    )
                    .await?;
                    write_rpc_error(writer, request_id, -32602, reason).await?;
                    answered_server_requests.insert(request_key);
                    return Ok(());
                }
            };
            let request = ApprovalRequest {
                request_id: request_id.clone(),
                method: method.to_owned(),
                kind,
                thread_id: thread_id.to_owned(),
                turn_id: turn_id.to_owned(),
                item_id: params
                    .get("itemId")
                    .or_else(|| params.get("callId"))
                    .and_then(Value::as_str)
                    .unwrap_or(&request_key)
                    .to_owned(),
                target,
                params,
            };
            send_event(
                sender,
                AgentEvent::ApprovalRequested {
                    request: request.clone(),
                },
            )
            .await?;
            let task_request_key = request_key.clone();
            let task_method = method.to_owned();
            let task_request = request.clone();
            let task = approval_tasks.spawn(async move {
                let decision = approvals.decide(&task_request).await;
                ApprovalResolution {
                    request_id: task_request.request_id,
                    request_key: task_request_key,
                    method: task_method,
                    decision,
                }
            });
            pending_approvals.insert(request_key.clone(), PendingApprovalTask { task, request });
        }
        ServerRequestKind::McpElicitation => {
            write_message(
                writer,
                &json!({
                    "id": request_id,
                    "result": {"action": "decline", "content": null}
                }),
            )
            .await?;
            send_server_request_warning(
                sender,
                method,
                "was declined because interactive MCP elicitation is not configured",
                "CODEX_MCP_ELICITATION_DECLINED",
            )
            .await?;
        }
        ServerRequestKind::UserInput | ServerRequestKind::DynamicTool => {
            reject_unsupported_server_request(
                writer,
                sender,
                request_id,
                method,
                "experimentalApi is disabled and no experimental handler is configured",
            )
            .await?;
        }
        ServerRequestKind::AuthTokenRefresh => {
            reject_unsupported_server_request(
                writer,
                sender,
                request_id,
                method,
                "external ChatGPT token management is not configured",
            )
            .await?;
        }
        ServerRequestKind::Attestation => {
            reject_unsupported_server_request(
                writer,
                sender,
                request_id,
                method,
                "requestAttestation capability is not enabled",
            )
            .await?;
        }
        ServerRequestKind::Unsupported => {
            reject_unsupported_server_request(
                writer,
                sender,
                request_id,
                method,
                "the request method is not supported by this adapter",
            )
            .await?;
        }
    }
    if !matches!(request_kind, ServerRequestKind::Approval(_)) {
        answered_server_requests.insert(request_key);
    }
    Ok(())
}

pub(super) fn approval_target(
    kind: ApprovalKind,
    params: &Value,
    approval_items: &HashMap<String, Value>,
) -> Result<NativeApprovalTarget, String> {
    let item = params
        .get("itemId")
        .and_then(Value::as_str)
        .and_then(|item_id| approval_items.get(item_id));
    match kind {
        ApprovalKind::CommandExecution => {
            if let Some(context) = params.get("networkApprovalContext") {
                let host = bounded_target_string(context.get("host"), "network host")?;
                let protocol = match context.get("protocol").and_then(Value::as_str) {
                    Some("http") => NativeNetworkProtocol::Http,
                    Some("https") => NativeNetworkProtocol::Https,
                    Some("socks5Tcp") => NativeNetworkProtocol::Socks5Tcp,
                    Some("socks5Udp") => NativeNetworkProtocol::Socks5Udp,
                    _ => return Err("network approval has an unsupported protocol".into()),
                };
                return Ok(NativeApprovalTarget::Network { host, protocol });
            }
            let command = approval_command(
                params
                    .get("command")
                    .or_else(|| item.and_then(|item| item.get("command"))),
            )?;
            let cwd = bounded_target_string(
                params
                    .get("cwd")
                    .or_else(|| item.and_then(|item| item.get("cwd"))),
                "command working directory",
            )?;
            Ok(NativeApprovalTarget::Command { command, cwd })
        }
        ApprovalKind::FileChange => {
            let grant_root = optional_bounded_target_string(params.get("grantRoot"), "grant root")?;
            let changes = item
                .and_then(|item| item.get("changes"))
                .and_then(Value::as_array)
                .map(|changes| project_file_changes(changes))
                .transpose()?
                .unwrap_or_default();
            if grant_root.is_none() && changes.is_empty() {
                return Err("file approval has no grant root or proposed file paths".into());
            }
            Ok(NativeApprovalTarget::FileChange {
                grant_root,
                changes,
            })
        }
        ApprovalKind::Permissions => Ok(NativeApprovalTarget::Permissions {
            cwd: bounded_target_string(params.get("cwd"), "permission working directory")?,
        }),
        ApprovalKind::LegacyCommand => {
            let command = approval_command(params.get("command"))?;
            let cwd = bounded_target_string(params.get("cwd"), "command working directory")?;
            Ok(NativeApprovalTarget::Command { command, cwd })
        }
        ApprovalKind::LegacyPatch => {
            let grant_root = optional_bounded_target_string(params.get("grantRoot"), "grant root")?;
            let changes = params
                .get("fileChanges")
                .and_then(Value::as_object)
                .map(|changes| {
                    changes
                        .iter()
                        .take(129)
                        .map(|(path, change)| {
                            let path = validate_bounded_string(path, "file path")?;
                            let kind = match change.get("type").and_then(Value::as_str) {
                                Some("add" | "create") => NativeApprovalFileChangeKind::Add,
                                Some("delete") => NativeApprovalFileChangeKind::Delete,
                                Some("update") | None => NativeApprovalFileChangeKind::Update,
                                Some(_) => {
                                    return Err(
                                        "file approval has an unsupported change kind".into()
                                    );
                                }
                            };
                            Ok(NativeApprovalFileChange { path, kind })
                        })
                        .collect::<Result<Vec<_>, String>>()
                })
                .transpose()?
                .unwrap_or_default();
            if changes.len() > 128 {
                return Err("file approval contains too many paths".into());
            }
            if grant_root.is_none() && changes.is_empty() {
                return Err("file approval has no grant root or proposed file paths".into());
            }
            Ok(NativeApprovalTarget::FileChange {
                grant_root,
                changes,
            })
        }
        ApprovalKind::Unsupported => Err("unsupported approval kind".into()),
    }
}

fn project_file_changes(changes: &[Value]) -> Result<Vec<NativeApprovalFileChange>, String> {
    if changes.is_empty() || changes.len() > 128 {
        return Err("file approval has no paths or contains too many paths".into());
    }
    changes
        .iter()
        .map(|change| {
            let path = bounded_target_string(change.get("path"), "file path")?;
            let kind = match change.get("kind").and_then(Value::as_str) {
                Some("add") => NativeApprovalFileChangeKind::Add,
                Some("delete") => NativeApprovalFileChangeKind::Delete,
                Some("update") => NativeApprovalFileChangeKind::Update,
                _ => return Err("file approval has an unsupported change kind".into()),
            };
            Ok(NativeApprovalFileChange { path, kind })
        })
        .collect()
}

fn approval_command(value: Option<&Value>) -> Result<String, String> {
    let arguments = match value {
        Some(Value::String(command)) => {
            let command = validate_bounded_string(command, "command")?;
            parse_approval_command(&command)?
        }
        Some(Value::Array(arguments)) if !arguments.is_empty() => arguments
            .iter()
            .map(|argument| {
                argument
                    .as_str()
                    .ok_or_else(|| "command approval contains a non-string argument".to_owned())
                    .and_then(|argument| validate_bounded_string(argument, "command argument"))
            })
            .collect::<Result<Vec<_>, _>>()?,
        _ => return Err("command approval has no concrete command".into()),
    };
    redact_approval_arguments(&arguments)
}

fn parse_approval_command(command: &str) -> Result<Vec<String>, String> {
    #[derive(Clone, Copy)]
    enum Quote {
        Single,
        Double,
    }

    let mut arguments = Vec::new();
    let mut argument = String::new();
    let mut argument_started = false;
    let mut quote = None;
    let mut escaped = false;
    for character in command.chars() {
        match quote {
            Some(Quote::Single) => {
                if character == '\'' {
                    quote = None;
                } else {
                    argument.push(character);
                }
            }
            Some(Quote::Double) => {
                if escaped {
                    argument.push(character);
                    escaped = false;
                } else if character == '\\' {
                    escaped = true;
                } else if character == '"' {
                    quote = None;
                } else {
                    argument.push(character);
                }
            }
            None => {
                if escaped {
                    argument.push(character);
                    escaped = false;
                } else if character == '\\' {
                    argument_started = true;
                    escaped = true;
                } else if character == '\'' {
                    argument_started = true;
                    quote = Some(Quote::Single);
                } else if character == '"' {
                    argument_started = true;
                    quote = Some(Quote::Double);
                } else if character.is_whitespace() {
                    if argument_started {
                        arguments.push(std::mem::take(&mut argument));
                        argument_started = false;
                    }
                } else {
                    argument_started = true;
                    argument.push(character);
                }
            }
        }
    }
    if quote.is_some() || escaped {
        return Err("command approval has unsupported quoting or escaping".into());
    }
    if argument_started {
        arguments.push(argument);
    }
    if arguments.is_empty() {
        return Err("command approval has no concrete command".into());
    }
    Ok(arguments)
}

fn redact_approval_arguments(arguments: &[String]) -> Result<String, String> {
    let mut redacted = Vec::with_capacity(arguments.len());
    let mut curl_headers = arguments.first().is_some_and(|argument| {
        argument.rsplit(['/', '\\']).next().is_some_and(|name| {
            name.eq_ignore_ascii_case("curl") || name.eq_ignore_ascii_case("curl.exe")
        })
    });
    let mut index = 0;
    while index < arguments.len() {
        let argument = &arguments[index];
        if curl_headers && argument == "--" {
            curl_headers = false;
            redacted.push(argument.clone());
            index += 1;
            continue;
        }
        if curl_headers && (argument == "-H" || argument.eq_ignore_ascii_case("--header")) {
            let Some(header) = arguments.get(index + 1) else {
                return Err("command approval has a header option without an argument".into());
            };
            redacted.push(argument.clone());
            redacted.push(redact_proven_header(header)?);
            index += 2;
            continue;
        }
        if curl_headers && let Some(header) = strip_ascii_case_prefix(argument, "--header=") {
            if header.is_empty() {
                return Err("command approval has an empty header argument".into());
            }
            redacted.push(format!("--header={}", redact_proven_header(header)?));
            index += 1;
            continue;
        }
        if curl_headers
            && let Some(header) = argument.strip_prefix("-H")
            && !header.is_empty()
        {
            redacted.push(format!("-H{}", redact_proven_header(header)?));
            index += 1;
            continue;
        }

        let token = redact_url_credentials(argument);
        let lower = token.to_ascii_lowercase();

        if sensitive_header_parts(&token).is_some() {
            return Err(
                "command approval has a sensitive header outside a proven header argument".into(),
            );
        }

        if let Some((key, value)) = token.split_once('=')
            && is_sensitive_key(key.to_ascii_lowercase().trim_start_matches('-'))
        {
            if value.trim().is_empty()
                || (key.to_ascii_lowercase().contains("authorization")
                    && is_authorization_scheme(value))
            {
                return Err(
                    "command approval has a sensitive value outside its argument boundary".into(),
                );
            }
            redacted.push(format!("{key}=[REDACTED]"));
            index += 1;
            continue;
        }

        if lower == "bearer" && index + 1 < arguments.len() {
            return Err("command approval has a bearer value outside its argument boundary".into());
        }

        if redact_inline_bearer(&token).is_some() {
            return Err(
                "command approval has a bearer credential outside a proven header argument".into(),
            );
        }

        if token.starts_with('-') && is_sensitive_key(lower.trim_start_matches('-')) {
            return Err(
                "command approval has a sensitive option outside its argument boundary".into(),
            );
        }

        redacted.push(token);
        index += 1;
    }
    let rendered = redacted
        .iter()
        .map(|argument| render_approval_argument(argument))
        .collect::<Vec<_>>()
        .join(" ");
    validate_bounded_string(&rendered, "redacted command")
}

fn redact_proven_header(header: &str) -> Result<String, String> {
    let header = redact_url_credentials(header);
    if let Some((name, value)) = sensitive_header_parts(&header) {
        let value = value.trim();
        if value.is_empty()
            || (name.trim().to_ascii_lowercase().contains("authorization")
                && is_authorization_scheme(value))
        {
            return Err(
                "command approval has a sensitive header value outside its argument boundary"
                    .into(),
            );
        }
        return Ok(format!("{}:[REDACTED]", name.trim()));
    }
    if let Some(projected) = redact_inline_bearer(&header) {
        return Ok(projected);
    }
    if is_sensitive_key(&header.trim().to_ascii_lowercase()) {
        return Err("command approval has a sensitive header without a value".into());
    }
    Ok(header)
}

fn render_approval_argument(argument: &str) -> String {
    if !argument.is_empty()
        && argument.chars().all(|character| {
            character.is_ascii_alphanumeric()
                || matches!(
                    character,
                    '-' | '_' | '.' | '/' | ':' | '@' | '%' | '+' | ',' | '=' | '[' | ']'
                )
        })
    {
        argument.to_owned()
    } else {
        serde_json::to_string(argument).expect("validated display strings are JSON serializable")
    }
}

fn sensitive_header_parts(header: &str) -> Option<(&str, &str)> {
    let (name, value) = header.split_once(':')?;
    let normalized = name.trim().to_ascii_lowercase();
    if !is_sensitive_key(&normalized) {
        return None;
    }
    Some((name, value))
}

fn is_authorization_scheme(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "bearer" | "basic" | "digest" | "negotiate" | "aws4-hmac-sha256"
    )
}

fn redact_inline_bearer(value: &str) -> Option<String> {
    let lower = value.to_ascii_lowercase();
    let mut search_start = 0;
    while let Some(relative) = lower[search_start..].find("bearer") {
        let start = search_start + relative;
        let end = start + "bearer".len();
        let left_boundary = start == 0 || !lower.as_bytes()[start - 1].is_ascii_alphanumeric();
        let right_boundary = end == lower.len()
            || lower.as_bytes()[end].is_ascii_whitespace()
            || matches!(lower.as_bytes()[end], b':' | b'=');
        if left_boundary && right_boundary && !value[end..].trim().is_empty() {
            return Some(format!("{} [REDACTED]", value[..end].trim_end()));
        }
        search_start = end;
    }
    None
}

fn strip_ascii_case_prefix<'a>(value: &'a str, prefix: &str) -> Option<&'a str> {
    value
        .get(..prefix.len())
        .filter(|candidate| candidate.eq_ignore_ascii_case(prefix))
        .map(|_| &value[prefix.len()..])
}

fn is_sensitive_key(key: &str) -> bool {
    [
        "token",
        "secret",
        "password",
        "passwd",
        "api-key",
        "api_key",
        "apikey",
        "credential",
        "authorization",
        "cookie",
    ]
    .iter()
    .any(|sensitive| key.contains(sensitive))
}

fn redact_url_credentials(token: &str) -> String {
    let mut redacted = token.to_owned();
    let mut search_start = 0;
    while let Some(relative_scheme_end) = redacted[search_start..].find("://") {
        let authority_start = search_start + relative_scheme_end + 3;
        let authority_end = redacted[authority_start..]
            .find(['/', '?', '#', ' ', '\t'])
            .map_or(redacted.len(), |relative| authority_start + relative);
        let authority = &redacted[authority_start..authority_end];
        let Some(relative_at) = authority.rfind('@') else {
            search_start = authority_end;
            continue;
        };
        redacted.replace_range(authority_start..authority_start + relative_at, "[REDACTED]");
        search_start = authority_start + "[REDACTED]@".len();
    }
    redacted
}

fn optional_bounded_target_string(
    value: Option<&Value>,
    field: &str,
) -> Result<Option<String>, String> {
    value
        .map(|value| bounded_target_string(Some(value), field))
        .transpose()
}

fn bounded_target_string(value: Option<&Value>, field: &str) -> Result<String, String> {
    let value = value
        .and_then(Value::as_str)
        .ok_or_else(|| format!("approval has no concrete {field}"))?;
    validate_bounded_string(value, field)
}

fn validate_bounded_string(value: &str, field: &str) -> Result<String, String> {
    if value.trim().is_empty() || value.len() > 4_096 || value.chars().any(char::is_control) {
        Err(format!(
            "approval {field} is empty or outside display limits"
        ))
    } else {
        Ok(value.to_owned())
    }
}

fn validate_approval_correlation(
    method: &str,
    params: &Value,
    thread_id: &str,
    turn_id: &str,
) -> Result<(), String> {
    if let Some(value) = params.get("threadId")
        && value.as_str() != Some(thread_id)
    {
        return Err(format!(
            "Codex approval {method} does not belong to the active thread"
        ));
    }
    if let Some(value) = params.get("turnId")
        && value.as_str() != Some(turn_id)
    {
        return Err(format!(
            "Codex approval {method} does not belong to the active turn"
        ));
    }
    if matches!(
        method,
        "item/commandExecution/requestApproval"
            | "item/fileChange/requestApproval"
            | "item/permissions/requestApproval"
    ) && (params.get("threadId").and_then(Value::as_str).is_none()
        || params.get("turnId").and_then(Value::as_str).is_none()
        || params.get("itemId").and_then(Value::as_str).is_none())
    {
        return Err(format!(
            "Codex approval {method} is missing threadId, turnId, or itemId"
        ));
    }
    Ok(())
}

fn server_request_kind(method: &str) -> ServerRequestKind {
    match method {
        "item/commandExecution/requestApproval" => {
            ServerRequestKind::Approval(ApprovalKind::CommandExecution)
        }
        "item/fileChange/requestApproval" => ServerRequestKind::Approval(ApprovalKind::FileChange),
        "item/permissions/requestApproval" => {
            ServerRequestKind::Approval(ApprovalKind::Permissions)
        }
        "execCommandApproval" => ServerRequestKind::Approval(ApprovalKind::LegacyCommand),
        "applyPatchApproval" => ServerRequestKind::Approval(ApprovalKind::LegacyPatch),
        "mcpServer/elicitation/request" => ServerRequestKind::McpElicitation,
        "item/tool/requestUserInput" => ServerRequestKind::UserInput,
        "item/tool/call" => ServerRequestKind::DynamicTool,
        "account/chatgptAuthTokens/refresh" => ServerRequestKind::AuthTokenRefresh,
        "attestation/generate" => ServerRequestKind::Attestation,
        _ => ServerRequestKind::Unsupported,
    }
}

fn server_request_key(request_id: &Value) -> Option<String> {
    match request_id {
        Value::String(value) => Some(format!("string:{value}")),
        Value::Number(value) if value.is_i64() => Some(format!("integer:{value}")),
        _ => None,
    }
}

async fn reject_unsupported_server_request<W>(
    writer: &mut W,
    sender: &mpsc::Sender<Result<AgentEvent, AdapterError>>,
    request_id: &Value,
    method: &str,
    reason: &str,
) -> Result<(), AdapterError>
where
    W: AsyncWrite + Unpin,
{
    send_server_request_warning(sender, method, reason, "CODEX_SERVER_REQUEST_UNSUPPORTED").await?;
    write_rpc_error(
        writer,
        request_id,
        -32601,
        format!("unsupported Codex server request {method}: {reason}"),
    )
    .await
}

async fn send_server_request_warning(
    sender: &mpsc::Sender<Result<AgentEvent, AdapterError>>,
    method: &str,
    reason: &str,
    code: &str,
) -> Result<(), AdapterError> {
    send_event(
        sender,
        AgentEvent::AdapterWarning {
            message: format!("Codex server request {method} {reason}"),
            retrying: false,
            code: Some(code.to_owned()),
        },
    )
    .await
}

async fn write_rpc_error<W>(
    writer: &mut W,
    request_id: &Value,
    code: i64,
    message: impl Into<String>,
) -> Result<(), AdapterError>
where
    W: AsyncWrite + Unpin,
{
    write_message(
        writer,
        &json!({
            "id": request_id,
            "error": {"code": code, "message": message.into()}
        }),
    )
    .await
}

pub(super) fn approval_response(
    method: &str,
    decision: ApprovalDecision,
) -> Result<Value, AdapterError> {
    if let ApprovalDecision::Raw(value) = decision {
        return Ok(value);
    }
    if decision == ApprovalDecision::Cancel {
        return Err(AdapterError::cancelled());
    }
    match method {
        "item/commandExecution/requestApproval" | "item/fileChange/requestApproval" => {
            let decision = match decision {
                ApprovalDecision::Accept => "accept",
                ApprovalDecision::AcceptForSession => "acceptForSession",
                ApprovalDecision::Decline => "decline",
                ApprovalDecision::Cancel => unreachable!("cancel interrupts the turn"),
                ApprovalDecision::Raw(_) => unreachable!(),
            };
            Ok(json!({"decision": decision}))
        }
        "execCommandApproval" | "applyPatchApproval" => {
            let decision = match decision {
                ApprovalDecision::Accept => "approved",
                ApprovalDecision::AcceptForSession => "approved_for_session",
                ApprovalDecision::Decline => "abort",
                ApprovalDecision::Cancel => unreachable!("cancel interrupts the turn"),
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
            ApprovalDecision::Decline => Ok(json!({"permissions": {}, "scope": "turn"})),
            ApprovalDecision::Cancel => unreachable!("cancel interrupts the turn"),
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
