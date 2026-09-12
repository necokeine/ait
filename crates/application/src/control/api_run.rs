//! Public control-store adapter for the existing provider-neutral `RunCoordinator`.
use super::{
    AgentProviderGateway, ApiError, Arc, ControlFilter, ControlStoreError, Digest, DomainError,
    ErrorCode, LocalControlService, MessageView, Mutex, Path, RunView, Sha256, Uuid, Value,
    WorkingSet, error, is_terminal_workspace_status, json, now, pending, recovery_error,
    release_session, validate_run_permission_ceiling,
};
use ait_contracts::ApiRunExecution;
use ait_domain::{
    Message, MessageId, MessageKind, MessageOrigin, MessageRole, ProjectedMessage, Run, RunAttempt,
    RunId, RunStatus, SubMessage, ToolExecution,
};
use ait_ports::{
    AgentInvocation, AgentResponse, ApprovalDecision, ApprovalRequest, CompletionResult, RunAgent,
    RunApproval, RunStore, RunStoreError, RunTool, ToolInvocation, ToolOutcome, ToolRecovery,
};
use ait_runtime::{RunCoordinator, SystemClock, UuidIds};
use async_trait::async_trait;

fn store_failure(_: impl std::fmt::Debug) -> RunStoreError {
    RunStoreError::Other("host Run persistence failed".into())
}
fn invalid(message: &str) -> ApiError {
    error(ErrorCode::InvalidRun, message, false)
}
fn conflict() -> RunStoreError {
    RunStoreError::Conflict("host Run state changed".into())
}
fn message_id(id: &str) -> Result<MessageId, RunStoreError> {
    Uuid::parse_str(id)
        .map(MessageId::new)
        .map_err(store_failure)
}
fn status(run: &Run) -> String {
    serde_json::to_value(run.status)
        .expect("status serializes")
        .as_str()
        .expect("status string")
        .into()
}

struct ProviderAgent {
    gateway: Arc<dyn AgentProviderGateway>,
    view: RunView,
    credential: String,
    names: Vec<String>,
}
#[async_trait]
impl RunAgent for ProviderAgent {
    async fn invoke(&self, request: AgentInvocation) -> Result<AgentResponse, DomainError> {
        self.gateway
            .complete_turn(
                &self.view.provider,
                &self.credential,
                &self.view.config,
                request,
                self.names.clone(),
            )
            .await
    }
}
struct NoTools;
#[async_trait]
impl RunTool for NoTools {
    fn requires_approval(&self, _: &str, _: &Value) -> bool {
        false
    }
    async fn execute(&self, _: ToolInvocation) -> Result<ToolOutcome, DomainError> {
        Err(DomainError::invariant(
            ErrorCode::ToolExecutionFailed,
            "host tool is unavailable",
        ))
    }
    async fn reconcile(&self, _: &ToolExecution) -> Result<ToolRecovery, DomainError> {
        Ok(ToolRecovery::Unknown)
    }
}
// There is no native API approval UI yet. Requests for wider access fail closed;
// they are never sent through the unrelated Codex operation/approval protocol.
struct DenyEscalation;
#[async_trait]
impl RunApproval for DenyEscalation {
    async fn decide(&self, _: ApprovalRequest) -> Result<ApprovalDecision, DomainError> {
        Ok(ApprovalDecision::Denied)
    }
}

impl LocalControlService {
    pub(super) async fn execute_api_run(
        &self,
        view: &RunView,
        cancellation: tokio_util::sync::CancellationToken,
    ) -> Result<RunView, ApiError> {
        if is_terminal_workspace_status(&view.status) {
            return Ok(view.clone());
        }
        if view.status == "cancelling" {
            cancellation.cancel();
        }
        let store = Arc::new(ControlRunStore {
            service: self.clone(),
            id: view.id.clone(),
            expected: Mutex::new(None),
        });
        store
            .initialize(view)
            .await
            .map_err(|_| recovery_error("could not initialize API Run"))?;
        let state = self.read_run_records(&view.id).await?.original;
        let preparation = (|| {
            validate_run_permission_ceiling(view.permission_profile, self.permission_limits)?;
            let project = state
                .projects
                .iter()
                .find(|p| p.id == view.project_id)
                .ok_or_else(|| {
                    DomainError::invariant(ErrorCode::InvalidProject, "Run Project is missing")
                })?;
            let tools = match &self.api_tools {
                Some(factory) => {
                    factory.create(Path::new(&project.workdir), view.permission_profile)?
                }
                None => Arc::new(NoTools) as Arc<dyn RunTool>,
            };
            let gateway = self.provider_gateway.clone().ok_or_else(|| {
                DomainError::invariant(
                    ErrorCode::InvalidConfiguration,
                    "provider gateway is unavailable",
                )
            })?;
            let credential = state
                .run_credentials
                .get(&view.id)
                .cloned()
                .ok_or_else(|| {
                    DomainError::invariant(
                        ErrorCode::InvalidConfiguration,
                        "provider credential is missing",
                    )
                })?;
            Ok::<_, DomainError>((tools, gateway, credential))
        })();
        let (tools, gateway, credential) = match preparation {
            Ok(parts) => parts,
            Err(failure) => {
                let mut run = store
                    .load_run(&RunId::new(&view.id))
                    .await
                    .map_err(|_| recovery_error("could not load API Run"))?;
                run.status = RunStatus::Failed;
                run.phase = ait_domain::RunPhase::Terminal;
                run.stop_reason = Some(ait_domain::RunStopReason::Failed);
                run.error = Some(failure);
                run.ended_at = Some(ait_domain::TimestampMs(now()));
                store
                    .save_run(run)
                    .await
                    .map_err(|_| recovery_error("could not persist API Run rejection"))?;
                return store.latest_view().await;
            }
        };
        let coordinator = RunCoordinator::new(
            store.clone(),
            Arc::new(ProviderAgent {
                gateway,
                view: view.clone(),
                credential,
                names: tools.executable_tools(),
            }),
            tools,
            Arc::new(DenyEscalation),
            Arc::new(SystemClock),
            Arc::new(UuidIds),
        );
        coordinator
            .drive(&RunId::new(&view.id), cancellation)
            .await
            .map_err(|_| {
                recovery_error(
                    "API Run stopped at a durable boundary; inspect execution before recovery",
                )
            })?;
        store.latest_view().await
    }
}
fn api_domain_error_reverse(e: DomainError) -> ApiError {
    error(e.code, e.message, e.retryable)
}
struct ControlRunStore {
    service: LocalControlService,
    id: String,
    expected: Mutex<Option<Run>>,
}
impl ControlRunStore {
    async fn latest_view(&self) -> Result<RunView, ApiError> {
        self.service
            .read_run_records(&self.id)
            .await?
            .original
            .runs
            .into_iter()
            .find(|r| r.id == self.id)
            .ok_or_else(|| invalid("Run disappeared"))
    }

    async fn initialize(&self, view: &RunView) -> Result<(), RunStoreError> {
        for _ in 0..8 {
            let loaded = self
                .service
                .read_run_records(&self.id)
                .await
                .map_err(store_failure)?;
            let mut state = loaded.original.clone();
            let target = state
                .runs
                .iter_mut()
                .find(|r| r.id == self.id)
                .ok_or_else(conflict)?;
            if target.execution.is_some() {
                return Ok(());
            }
            let now = ait_domain::TimestampMs(now());
            let run = Run {
                id: RunId::new(&view.id),
                project_id: ait_domain::ProjectId::new(&view.project_id),
                base_message_id: message_id(&view.base_message_id)?,
                last_message_id: None,
                follow_session_id: view.session_id.as_ref().map(ait_domain::SessionId::new),
                agent_id: ait_domain::AgentId::new(&view.agent_id),
                agent_revision: view.agent_revision,
                agent_snapshot: ait_domain::AgentConfigSnapshot {
                    agent_id: ait_domain::AgentId::new(&view.agent_id),
                    revision: view.agent_revision,
                    driver_type: format!("{:?}", view.provider.kind),
                    connection_name: view.config.provider_id.clone(),
                    model: view.config.model.clone(),
                    endpoint: view.provider.url.clone(),
                    capabilities: std::collections::BTreeSet::default(),
                    default_parameters: ait_domain::DomainMetadata::default(),
                    tool_policy: ait_domain::ToolPolicy::default(),
                    config_digest: format!(
                        "{:x}",
                        Sha256::digest(serde_json::to_vec(&view.config).map_err(store_failure)?)
                    ),
                },
                trigger: if view.trigger == "cron" {
                    ait_domain::RunTrigger::Cron
                } else {
                    ait_domain::RunTrigger::Manual
                },
                cron_id: view.cron_id.as_ref().map(ait_domain::CronId::new),
                scheduled_at: view.scheduled_at.map(ait_domain::TimestampMs),
                status: RunStatus::Queued,
                phase: ait_domain::RunPhase::Queued,
                stop_reason: None,
                error: None,
                step_count: 0,
                budget: ait_domain::RunBudget {
                    max_steps: 128,
                    token_budget: Some(1_000_000),
                    cost_budget: None,
                    max_runtime: Some(ait_domain::DurationMs(300_000)),
                },
                usage: ait_domain::RunUsage::default(),
                attempt_count: 0,
                compaction_count: 0,
                retry_policy: ait_domain::RetryPolicy {
                    max_attempts: 3,
                    initial_delay: ait_domain::DurationMs(100),
                    max_delay: ait_domain::DurationMs(1000),
                },
                next_retry_at: None,
                checkpoint_id: None,
                queue_version: 0,
                queue_cursor: 0,
                dedupe_key: None,
                started_at: None,
                ended_at: None,
                created_at: now,
            };
            run.validate().map_err(store_failure)?;
            target.execution = Some(Box::new(ApiRunExecution {
                run,
                attempts: Vec::new(),
                tools: Vec::new(),
            }));
            match self
                .service
                .persist_records(&loaded, &state, Vec::new())
                .await
            {
                Ok(()) => return Ok(()),
                Err(ControlStoreError::Conflict) => {}
                Err(e) => return Err(store_failure(e)),
            }
        }
        Err(conflict())
    }
    async fn state(&self) -> Result<ApiRunExecution, RunStoreError> {
        self.service
            .read_run_records(&self.id)
            .await
            .map_err(store_failure)?
            .original
            .runs
            .into_iter()
            .find(|r| r.id == self.id)
            .and_then(|r| r.execution)
            .map(|e| *e)
            .ok_or_else(conflict)
    }
    async fn persist(
        &self,
        mut run: Run,
        attempt: Option<RunAttempt>,
        tool: Option<ToolExecution>,
        message: Option<Message>,
    ) -> Result<Run, RunStoreError> {
        run.validate().map_err(store_failure)?;
        let expected = self
            .expected
            .lock()
            .map_err(store_failure)?
            .clone()
            .ok_or_else(conflict)?;
        for _ in 0..8 {
            let loaded = self
                .service
                .read_run_records(&self.id)
                .await
                .map_err(store_failure)?;
            let mut state = loaded.original.clone();
            let index = state
                .runs
                .iter()
                .position(|r| r.id == self.id)
                .ok_or_else(conflict)?;
            let mut view = state.runs[index].clone();
            if view.status == "cancelling" && run.status == RunStatus::Completed {
                run.status = RunStatus::Cancelled;
                run.stop_reason = Some(ait_domain::RunStopReason::Cancelled);
            }
            let execution = view.execution.as_mut().ok_or_else(conflict)?;
            if execution.run != expected
                || (is_terminal_workspace_status(&view.status) && view.status != status(&expected))
            {
                return Err(conflict());
            }
            if let Some(attempt) = &attempt {
                attempt.validate().map_err(store_failure)?;
                execution.attempts.retain(|a| a.id != attempt.id);
                execution.attempts.push(attempt.clone());
            }
            if let Some(tool) = &tool {
                tool.validate().map_err(store_failure)?;
                validate_tool_child(&state, &run, tool, message.as_ref())?;
                if execution
                    .tools
                    .iter()
                    .any(|t| t.id != tool.id && t.call_id == tool.call_id)
                {
                    return Err(conflict());
                }
                execution.tools.retain(|t| t.id != tool.id);
                execution.tools.push(tool.clone());
            }
            execution.run = run.clone();
            if let Some(message) = &message {
                append_projection(&mut state, &view, &run, &expected, message)?;
            }
            view.status = status(&run);
            view.phase = Some(
                serde_json::to_value(run.phase)
                    .map_err(store_failure)?
                    .as_str()
                    .ok_or_else(conflict)?
                    .into(),
            );
            view.last_message_id = run.last_message_id.map(|id| id.as_uuid().to_string());
            view.error = run.error.clone().map(api_domain_error_reverse);
            if run.status.is_terminal() {
                release_session(&mut state, &view);
            }
            state.runs[index] = view;
            // Run events retain the public shape; pending() excludes execution payloads.
            let events = vec![pending(
                "run.updated",
                Some(self.id.clone()),
                &state.runs[index],
            )];
            match self.service.persist_records(&loaded, &state, events).await {
                Ok(()) => {
                    *self.expected.lock().map_err(store_failure)? = Some(run.clone());
                    return Ok(run);
                }
                Err(ControlStoreError::Conflict) => {}
                Err(e) => return Err(store_failure(e)),
            }
        }
        Err(conflict())
    }
}
#[async_trait]
impl RunStore for ControlRunStore {
    async fn load_run(&self, _: &RunId) -> Result<Run, RunStoreError> {
        let run = self.state().await?.run;
        *self.expected.lock().map_err(store_failure)? = Some(run.clone());
        Ok(run)
    }
    async fn load_message_path(
        &self,
        head: &MessageId,
    ) -> Result<Vec<ProjectedMessage>, RunStoreError> {
        let state = self
            .service
            .read_records(vec![ControlFilter::message_ancestors(
                head.as_uuid().to_string(),
            )])
            .await
            .map_err(store_failure)?
            .original;
        let mut path = Vec::new();
        let mut cursor = Some(head.as_uuid().to_string());
        while let Some(id) = cursor.as_deref() {
            let m = state
                .messages
                .iter()
                .find(|m| m.id == id)
                .ok_or_else(conflict)?;
            let message =
                if let Some(native) = m.data.as_ref().and_then(|d| d.get("native_message")) {
                    serde_json::from_value(native.clone()).map_err(store_failure)?
                } else {
                    Message {
                        id: message_id(&m.id)?,
                        project_id: ait_domain::ProjectId::new(&m.project_id),
                        parent_message_id: m
                            .parent_message_id
                            .as_deref()
                            .map(message_id)
                            .transpose()?,
                        role: match m.role.as_str() {
                            "assistant" => MessageRole::Assistant,
                            "system" => MessageRole::System,
                            _ => MessageRole::User,
                        },
                        kind: MessageKind::Standard,
                        origin: if m.role == "system" {
                            MessageOrigin::Project
                        } else if m.git_commit.is_some() {
                            MessageOrigin::Human
                        } else {
                            MessageOrigin::Agent
                        },
                        sub_messages: vec![SubMessage::Text {
                            text: m.text.clone().unwrap_or_default(),
                        }],
                        created_by_session_id: None,
                        run_id: None,
                        run_seq: None,
                        tool_result: None,
                        git_commit: m
                            .git_commit
                            .as_ref()
                            .map(|s| ait_domain::GitCommit::parse(s.clone()))
                            .transpose()
                            .map_err(store_failure)?,
                        metadata: ait_domain::DomainMetadata::default(),
                        created_at: ait_domain::TimestampMs(m.created_at),
                    }
                };
            cursor.clone_from(&m.parent_message_id);
            path.push(ProjectedMessage::Visible(message));
        }
        path.reverse();
        Ok(path)
    }
    async fn load_attempts(&self, _: &RunId) -> Result<Vec<RunAttempt>, RunStoreError> {
        Ok(self.state().await?.attempts)
    }
    async fn load_tool_executions(
        &self,
        _: &RunId,
        assistant: &MessageId,
    ) -> Result<Vec<ToolExecution>, RunStoreError> {
        Ok(self
            .state()
            .await?
            .tools
            .into_iter()
            .filter(|t| &t.assistant_message_id == assistant)
            .collect())
    }
    async fn save_run(&self, run: Run) -> Result<Run, RunStoreError> {
        self.persist(run, None, None, None).await
    }
    async fn save_attempt(&self, run: Run, attempt: RunAttempt) -> Result<Run, RunStoreError> {
        self.persist(run, Some(attempt), None, None).await
    }
    async fn append_message(&self, run: Run, message: Message) -> Result<Run, RunStoreError> {
        self.persist(run, None, None, Some(message)).await
    }
    async fn save_tool_execution(
        &self,
        run: Run,
        tool: ToolExecution,
    ) -> Result<Run, RunStoreError> {
        self.persist(run, None, Some(tool), None).await
    }
    async fn append_tool_result(
        &self,
        run: Run,
        tool: ToolExecution,
        message: Message,
    ) -> Result<Run, RunStoreError> {
        self.persist(run, None, Some(tool), Some(message)).await
    }
    async fn try_complete(
        &self,
        run: Run,
        version: u64,
    ) -> Result<CompletionResult, RunStoreError> {
        let state = self.state().await?;
        if state.run.queue_version != version {
            return Ok(CompletionResult::QueueChanged(state.run));
        }
        if state
            .tools
            .iter()
            .any(|t| !t.status.is_terminal() || t.tool_result_message_id.is_none())
        {
            return Err(conflict());
        }
        Ok(CompletionResult::Completed(self.save_run(run).await?))
    }
    async fn drain_queue(&self, run: Run) -> Result<Run, RunStoreError> {
        if run.queue_version != run.queue_cursor {
            return Err(conflict());
        }
        Ok(run)
    }
}

fn append_projection(
    state: &mut WorkingSet,
    view: &RunView,
    run: &Run,
    expected: &Run,
    message: &Message,
) -> Result<(), RunStoreError> {
    message.validate().map_err(store_failure)?;
    if message.run_id.as_ref() != Some(&run.id)
        || message.run_seq != Some(run.step_count)
        || run.step_count != expected.step_count + 1
        || message.parent_message_id
            != Some(expected.last_message_id.unwrap_or(expected.base_message_id))
        || state
            .messages
            .iter()
            .any(|m| m.id == message.id.as_uuid().to_string())
    {
        return Err(conflict());
    }
    if let Some(result) = &message.tool_result
        && state.messages.iter().any(|m| {
            m.data.as_ref().is_some_and(|data| {
                data["native_message"]["tool_result"]["call_id"] == result.call_id
                    && data["native_message"]["run_id"] == run.id.as_str()
            })
        })
    {
        return Err(conflict());
    }
    let text = message
        .sub_messages
        .iter()
        .filter_map(|p| match p {
            SubMessage::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<String>();
    state.messages.push(MessageView {
        id: message.id.as_uuid().to_string(),
        project_id: view.project_id.clone(),
        parent_message_id: message.parent_message_id.map(|id| id.as_uuid().to_string()),
        role: serde_json::to_value(message.role)
            .map_err(store_failure)?
            .as_str()
            .ok_or_else(conflict)?
            .into(),
        kind: if message.kind == MessageKind::ToolResult {
            "tool_result"
        } else {
            "standard"
        }
        .into(),
        text: (!text.is_empty()).then_some(text),
        created_at: message.created_at.0,
        git_commit: None,
        data: Some(json!({"native_message":message,"agent_revision":run.agent_revision})),
    });
    if let Some(session) = state
        .sessions
        .iter_mut()
        .find(|s| Some(&s.id) == view.session_id.as_ref())
    {
        if session.active_run_id.as_deref() != Some(&view.id)
            || session.current_message_id
                != expected
                    .last_message_id
                    .unwrap_or(expected.base_message_id)
                    .as_uuid()
                    .to_string()
        {
            return Err(conflict());
        }
        session.current_message_id = message.id.as_uuid().to_string();
        session.version += 1;
    }

    Ok(())
}

fn validate_tool_child(
    state: &WorkingSet,
    run: &Run,
    tool: &ToolExecution,
    result: Option<&Message>,
) -> Result<(), RunStoreError> {
    if tool.run_id != run.id {
        return Err(conflict());
    }
    let native = state
        .messages
        .iter()
        .find(|m| m.id == tool.assistant_message_id.as_uuid().to_string())
        .and_then(|m| m.data.as_ref())
        .and_then(|d| d.get("native_message"))
        .ok_or_else(conflict)?;
    let assistant: Message = serde_json::from_value(native.clone()).map_err(store_failure)?;
    let Some(SubMessage::ToolUse(proposal)) =
        assistant.sub_messages.get(tool.tool_use_index as usize)
    else {
        return Err(conflict());
    };
    if assistant.run_id.as_ref() != Some(&run.id)
        || proposal.call_id != tool.call_id
        || proposal.tool_name != tool.tool_name
        || serde_json::from_str::<Value>(&proposal.arguments).map_err(store_failure)?
            != tool.arguments
    {
        return Err(conflict());
    }
    if let Some(result) = result {
        if tool.tool_result_message_id != Some(result.id) {
            return Err(conflict());
        }
        let mut unlinked = tool.clone();
        unlinked.tool_result_message_id = None;
        unlinked
            .validate_result_message(result)
            .map_err(store_failure)?;
    }
    Ok(())
}

/// Keep the public projection and canonical aggregate consistent on policy-driven recovery stops.
pub(super) fn interrupt(view: &mut RunView) {
    let Some(execution) = view.execution.as_mut() else {
        return;
    };
    let (status, reason) = if view.status == "cancelled" {
        (RunStatus::Cancelled, ait_domain::RunStopReason::Cancelled)
    } else {
        (RunStatus::Failed, ait_domain::RunStopReason::Failed)
    };
    execution.run.status = status;
    execution.run.phase = ait_domain::RunPhase::Terminal;
    execution.run.stop_reason = Some(reason);
    execution.run.next_retry_at = None;
    execution.run.ended_at = Some(ait_domain::TimestampMs(now()));
    execution.run.error = view
        .error
        .as_ref()
        .map(|e| DomainError::invariant(e.code, &e.message));
    for attempt in &mut execution.attempts {
        if attempt.status == ait_domain::RunAttemptStatus::Running {
            attempt.status = if status == RunStatus::Cancelled {
                ait_domain::RunAttemptStatus::Cancelled
            } else {
                ait_domain::RunAttemptStatus::Failed
            };
            attempt.ended_at = execution.run.ended_at;
            attempt.error.clone_from(&execution.run.error);
        }
    }
}
