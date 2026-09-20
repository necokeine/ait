//! Native Thread admission, process ownership and authoritative terminal reads.
use std::{path::Path, sync::Arc, time::Duration};

use ait_domain::{DomainError, ErrorCode, SandboxAccess};
use ait_ports::{
    CodexPreparedThread, CodexThreadConnection, CodexThreadInvocation, CodexThreadSnapshot,
    CodexThreadWriter, WorkspaceProgressEvent, WorkspaceProgressReporter,
};
use async_trait::async_trait;
use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, BufReader, Lines},
    process::{Child, ChildStdin, ChildStdout},
    sync::mpsc,
    task::JoinHandle,
};

use super::{
    CodexAppServerAdapter, WorkspaceApprovalBridge, codex_thread_write_error, domain_error,
    protocol::{
        ClientInfo, drive_turn_protocol, initialize_protocol, read_thread_history,
        wait_for_response, write_message,
    },
    report_item,
};
use crate::{AgentEvent, AgentRunRequest, ApprovalPolicy, SandboxMode};

struct NativeConnection {
    child: Option<Child>,
    stderr: Option<JoinHandle<()>>,
    lines: Lines<BufReader<ChildStdout>>,
    stdin: ChildStdin,
    invocation: CodexThreadInvocation,
    thread_id: String,
    resumed: Option<CodexPreparedThread>,
    next_read_id: i64,
    fresh: bool,
    limits: Option<super::CodexExecutionLimits>,
}

impl Drop for NativeConnection {
    fn drop(&mut self) {
        if let Some(task) = self.stderr.take() {
            task.abort();
        }
        if let Some(mut child) = self.child.take() {
            let _ = child.start_kill();
            if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                runtime.spawn(async move {
                    let _ = child.wait().await;
                });
            }
        }
    }
}

#[async_trait]
impl CodexThreadWriter for CodexAppServerAdapter {
    async fn open(
        &self,
        request: CodexThreadInvocation,
    ) -> Result<Box<dyn CodexThreadConnection>, DomainError> {
        if !request.cwd.is_absolute()
            || request
                .thread_id
                .as_ref()
                .is_some_and(|id| id.trim().is_empty())
            || (request.thread_id.is_some() && request.developer_instructions.is_some())
        {
            return Err(domain_error(
                ErrorCode::InvalidConfiguration,
                "invalid native Thread admission",
                false,
            ));
        }
        let mut child = self.spawn_process(&request.cwd).map_err(pre_send_error)?;
        let stdout = child.stdout.take().expect("spawned app-server stdout pipe");
        let stdin = child.stdin.take().expect("spawned app-server stdin pipe");
        let stderr = child.stderr.take().map(|stderr| {
            tokio::spawn(async move {
                let mut lines = BufReader::new(stderr).lines();
                while let Ok(Some(_)) = lines.next_line().await {}
            })
        });
        let mut connection = NativeConnection {
            child: Some(child),
            stderr,
            lines: BufReader::new(stdout).lines(),
            stdin,
            resumed: None,
            thread_id: String::new(),
            next_read_id: 1000,
            fresh: request.thread_id.is_none(),
            invocation: request,
            limits: self.config.execution_limits,
        };
        let cancellation = connection.invocation.cancellation.clone();
        let admission = tokio::select! {
            () = cancellation.cancelled() => Err(domain_error(ErrorCode::RunCancelled, "native admission cancelled", false)),
            result = tokio::time::timeout(Duration::from_secs(30), connection.initialize(ClientInfo {
                name: self.config.client_name.clone(), title: self.config.client_title.clone(), version: self.config.client_version.clone(),
            })) => result.unwrap_or_else(|_| Err(domain_error(ErrorCode::CodexThreadNotSynced, "native writer admission timed out", true))),
        };
        if let Err(failure) = admission {
            connection.close().await;
            return Err(failure);
        }
        Ok(Box::new(connection))
    }
}

impl NativeConnection {
    async fn initialize(&mut self, client: ClientInfo) -> Result<(), DomainError> {
        initialize_protocol(&mut self.lines, &mut self.stdin, client)
            .await
            .map_err(pre_send_error)?;
        let request = &self.invocation;
        let mut params = json!({
            "model":request.model,
            "sandbox": sandbox(request.permission_profile.sandbox).as_wire_value(),
            "approvalPolicy": approval(&self.invocation).as_wire_value(), "approvalsReviewer":"user",
        });
        let method = if let Some(id) = &request.thread_id {
            // Preserve native cwd and developer instructions on continuation.
            params["threadId"] = json!(id);
            "thread/resume"
        } else {
            params["cwd"] = json!(request.cwd);
            params["ephemeral"] = json!(false);
            if let Some(instructions) = &request.developer_instructions {
                params["developerInstructions"] = json!(instructions);
            }
            "thread/start"
        };
        write_message(
            &mut self.stdin,
            &json!({"id": 1, "method": method, "params": params}),
        )
        .await
        .map_err(pre_send_error)?;
        let (result, _) = wait_for_response(&mut self.lines, 1)
            .await
            .map_err(pre_send_error)?;
        let (model, model_provider, reasoning_effort) = validate_resume(&result, request)?;
        result
            .pointer("/thread/id")
            .and_then(Value::as_str)
            .expect("validated native Thread identity")
            .clone_into(&mut self.thread_id);
        let history = if self.fresh {
            let mut history: CodexThreadSnapshot = serde_json::from_value(result["thread"].clone())
                .map_err(|_| {
                    domain_error(
                        ErrorCode::CodexHistorySchemaUnsupported,
                        "invalid thread/start metadata",
                        false,
                    )
                })?;
            if !history.turns.is_empty()
                || history.status.get("type").and_then(Value::as_str) != Some("idle")
            {
                return Err(domain_error(
                    ErrorCode::CodexThreadNotSynced,
                    "new Thread is not empty and idle",
                    false,
                ));
            }
            history.writer_confirmed = true;
            history
        } else {
            self.read().await?
        };
        if !history.writer_confirmed {
            return Err(domain_error(
                ErrorCode::CodexThreadActiveElsewhere,
                "resumed Thread is not idle",
                false,
            ));
        }
        self.resumed = Some(CodexPreparedThread {
            history,
            model,
            model_provider,
            reasoning_effort,
        });
        Ok(())
    }

    fn request(&self) -> AgentRunRequest {
        let request = &self.invocation;
        AgentRunRequest {
            request_id: request.request_id.clone(),
            model: Some(self.prepared().model.clone()),
            reasoning_effort: self.prepared().reasoning_effort.clone(),
            project_instructions: None,
            prompt: request.prompt.clone(),
            cwd: request.cwd.clone(),
            resume_thread_id: Some(self.thread_id.clone()),
            ephemeral: false,
            sandbox: sandbox(request.permission_profile.sandbox),
            approval_policy: approval(request),
            approval_handler: None,
            output_schema: None,
            cancellation: request.cancellation.clone(),
        }
    }

    async fn final_history(&mut self) -> Result<CodexThreadSnapshot, DomainError> {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        let mut interrupted = false;
        loop {
            let history = tokio::time::timeout_at(deadline, self.read())
                .await
                .map_err(|_| {
                    domain_error(
                        ErrorCode::CodexInputOutcomeUnknown,
                        "native final history is not confirmed",
                        false,
                    )
                })??;
            if history.writer_confirmed {
                return Ok(history);
            }
            if self.invocation.cancellation.is_cancelled() && !interrupted {
                for turn in history
                    .turns
                    .iter()
                    .filter(|turn| turn.status == "inProgress")
                {
                    write_message(
                        &mut self.stdin,
                        &json!({"id":900, "method":"turn/interrupt", "params":{
                            "threadId":self.thread_id, "turnId":turn.id,
                        }}),
                    )
                    .await
                    .map_err(codex_thread_write_error)?;
                    tokio::time::timeout_at(deadline, wait_for_response(&mut self.lines, 900))
                        .await
                        .map_err(|_| {
                            domain_error(
                                ErrorCode::CodexInputOutcomeUnknown,
                                "native interruption is not confirmed",
                                false,
                            )
                        })?
                        .map_err(codex_thread_write_error)?;
                }
                interrupted = true;
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(domain_error(
                    ErrorCode::CodexInputOutcomeUnknown,
                    "native Turn remains active",
                    false,
                ));
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
}

#[async_trait]
impl CodexThreadConnection for NativeConnection {
    fn prepared(&self) -> &CodexPreparedThread {
        self.resumed
            .as_ref()
            .expect("connection is exposed only after verified resume")
    }

    async fn start(
        &mut self,
        progress: Arc<dyn WorkspaceProgressReporter>,
    ) -> Result<CodexThreadSnapshot, DomainError> {
        if self.invocation.cancellation.is_cancelled() {
            return Err(domain_error(
                ErrorCode::RunCancelled,
                "native input cancelled before sending",
                false,
            ));
        }
        self.fresh = false;
        let request = self.request();
        let cancellation = request.cancellation.clone();
        let approvals = Arc::new(WorkspaceApprovalBridge {
            run_id: self.invocation.request_id.clone(),
            sandbox: self.invocation.permission_profile.sandbox,
            cwd: self.invocation.cwd.clone(),
            approvals: self.invocation.approvals.clone(),
        });
        let (sender, mut receiver) = mpsc::channel(128);
        let protocol = async {
            let result = tokio::select! {
                biased;
                result = drive_turn_protocol(&mut self.lines, &mut self.stdin, request, self.thread_id.clone(), approvals, &sender) => result,
                () = cancellation.cancelled() => Err(crate::AdapterError::cancelled()),
            };
            drop(sender);
            result
        };
        let limits = self.limits;
        let mut limit_failure = None;
        let mut meter = super::budget::Meter::default();
        let progress = async {
            while let Some(Ok(event)) = receiver.recv().await {
                if let Err(failure) = meter.check(&event, limits) {
                    limit_failure = Some(failure);
                    cancellation.cancel();
                    continue;
                }
                match event {
                    AgentEvent::MessageDelta { item_id, delta } => {
                        progress
                            .report(WorkspaceProgressEvent::TextDelta { id: item_id, delta })
                            .await;
                    }
                    AgentEvent::ItemStarted { item } => {
                        report_item(Some(&progress), &item, false).await;
                    }
                    AgentEvent::ItemCompleted { item } => {
                        report_item(Some(&progress), &item, true).await;
                    }
                    AgentEvent::AdapterWarning {
                        message,
                        retrying,
                        code,
                    } => {
                        progress
                            .report(WorkspaceProgressEvent::Warning {
                                message,
                                retrying,
                                code,
                            })
                            .await;
                    }
                    _ => {}
                }
            }
        };
        let (result, ()) = tokio::join!(protocol, progress);
        if let Some(failure) = limit_failure {
            // Interrupt/reconcile before reaping. Never convert the interrupted model
            // result into a successful Run or trigger Git finalization.
            let _ = self.final_history().await;
            return Err(failure.into());
        }
        if result.as_ref().is_err_and(|failure| failure.code.is_some()) {
            return Err(pre_send_error(
                result.expect_err("checked rejected turn/start"),
            ));
        }
        // A terminal notification is never itself a full history snapshot.
        let history = self.final_history().await.map_err(|failure| {
            domain_error(ErrorCode::CodexInputOutcomeUnknown, failure.message, false)
        })?;
        let found = history
            .turns
            .iter()
            .flat_map(|turn| &turn.items)
            .any(|item| {
                item.get("clientId").and_then(Value::as_str) == Some(&self.invocation.request_id)
            });
        if !found && let Err(failure) = result {
            return Err(if failure.code.is_some() {
                pre_send_error(failure)
            } else {
                domain_error(ErrorCode::CodexInputOutcomeUnknown, failure.message, false)
            });
        }
        Ok(history)
    }

    async fn read(&mut self) -> Result<CodexThreadSnapshot, DomainError> {
        if self.fresh {
            return Ok(self.prepared().history.clone());
        }
        let mut history = read_thread_history(
            &mut self.lines,
            &mut self.stdin,
            &self.thread_id,
            &mut self.next_read_id,
        )
        .await
        .map_err(codex_thread_write_error)?;
        if Path::new(&history.cwd) != self.invocation.cwd {
            return Err(domain_error(
                ErrorCode::CodexThreadBindingConflict,
                "native Thread cwd changed",
                false,
            ));
        }
        history.writer_confirmed =
            history.status.get("type").and_then(Value::as_str) == Some("idle")
                && history.turns.iter().all(|turn| {
                    matches!(turn.status.as_str(), "completed" | "failed" | "interrupted")
                });
        Ok(history)
    }

    async fn close(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.start_kill();
            let _ = child.wait().await;
        }
        if let Some(task) = self.stderr.take() {
            task.abort();
        }
    }
}

fn validate_resume(
    result: &Value,
    request: &CodexThreadInvocation,
) -> Result<(String, String, Option<String>), DomainError> {
    let required = |field: &str| result.get(field).and_then(Value::as_str);
    let expected_sandbox = match request.permission_profile.sandbox {
        SandboxAccess::ReadOnly => "readOnly",
        SandboxAccess::WorkspaceWrite => "workspaceWrite",
        SandboxAccess::FullAccess => "dangerFullAccess",
    };
    let thread_id = result.pointer("/thread/id").and_then(Value::as_str);
    if thread_id.is_none_or(str::is_empty)
        || request
            .thread_id
            .as_deref()
            .is_some_and(|expected| thread_id != Some(expected))
        || required("model") != Some(&request.model)
        || required("cwd").map(Path::new) != Some(request.cwd.as_path())
        || required("approvalPolicy") != Some(approval(request).as_wire_value())
        || required("approvalsReviewer") != Some("user")
        || result.pointer("/sandbox/type").and_then(Value::as_str) != Some(expected_sandbox)
        || required("modelProvider").is_none_or(str::is_empty)
    {
        return Err(domain_error(
            ErrorCode::CodexThreadCapabilityUnsupported,
            "resumed Codex identity, model, cwd or approval policy differs from admission",
            false,
        ));
    }
    let effort = request
        .reasoning_effort
        .clone()
        .or_else(|| required("reasoningEffort").map(str::to_owned));
    Ok((
        request.model.clone(),
        required("modelProvider")
            .expect("validated provider")
            .to_owned(),
        effort,
    ))
}

fn sandbox(access: SandboxAccess) -> SandboxMode {
    match access {
        SandboxAccess::ReadOnly => SandboxMode::ReadOnly,
        SandboxAccess::WorkspaceWrite => SandboxMode::WorkspaceWrite,
        SandboxAccess::FullAccess => SandboxMode::DangerFullAccess,
    }
}
fn approval(request: &CodexThreadInvocation) -> ApprovalPolicy {
    match request.permission_profile.approval {
        ait_domain::ApprovalMode::OnRequest => ApprovalPolicy::OnRequest,
        ait_domain::ApprovalMode::UntrustedOnly => ApprovalPolicy::Untrusted,
    }
}
fn pre_send_error(failure: crate::AdapterError) -> DomainError {
    let mut failure = codex_thread_write_error(failure);
    if failure.code == ErrorCode::CodexInputOutcomeUnknown {
        failure.code = ErrorCode::CodexInputNotAccepted;
    }
    failure
}
#[cfg(test)]
mod tests;
