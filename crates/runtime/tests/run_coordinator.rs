//! Contract tests for the persisted Run coordinator and recovery loop.

use std::{
    collections::{BTreeMap, BTreeSet, HashMap, VecDeque},
    sync::{
        Arc, Mutex,
        atomic::{AtomicI64, AtomicU64, Ordering},
    },
};

use ait_domain::{
    AgentCapability, AgentConfigSnapshot, AgentId, CostMicros, DomainError, DomainMetadata,
    DurationMs, ErrorCode, Message, MessageId, MessageKind, MessageOrigin, MessageRole, ProjectId,
    ProjectedMessage, RetryPolicy, Run, RunAttempt, RunAttemptId, RunBudget, RunId, RunPhase,
    RunStatus, RunStopReason, RunTrigger, RunUsage, SubMessage, TimestampMs, ToolApprovalStatus,
    ToolExecution, ToolExecutionId, ToolExecutionStatus, ToolPolicy, ToolResultStatus, ToolUse,
};
use ait_ports::{
    AgentInvocation, AgentResponse, ApprovalDecision, ApprovalRequest, CompletionResult, RunAgent,
    RunApproval, RunClock, RunIdGenerator, RunStore, RunStoreError, RunTool, ToolInvocation,
    ToolOutcome, ToolRecovery,
};
use ait_runtime::{DriveOutcome, RunCoordinator};
use async_trait::async_trait;
use serde_json::json;
use tokio_util::sync::CancellationToken;

#[derive(Default)]
struct MemoryState {
    run: Option<Run>,
    messages: HashMap<MessageId, Message>,
    attempts: Vec<RunAttempt>,
    tools: Vec<ToolExecution>,
    commits: Vec<String>,
}

#[derive(Default)]
struct MemoryStore(Mutex<MemoryState>);

impl MemoryStore {
    fn seeded(run: Run, messages: Vec<Message>) -> Self {
        Self(Mutex::new(MemoryState {
            run: Some(run),
            messages: messages
                .into_iter()
                .map(|message| (message.id, message))
                .collect(),
            ..MemoryState::default()
        }))
    }

    fn snapshot(&self) -> MemoryStateSnapshot {
        let state = self.0.lock().unwrap();
        MemoryStateSnapshot {
            run: state.run.clone().unwrap(),
            messages: state.messages.values().cloned().collect(),
            attempts: state.attempts.clone(),
            tools: state.tools.clone(),
            commits: state.commits.clone(),
        }
    }

    fn insert_tool(&self, execution: ToolExecution) {
        self.0.lock().unwrap().tools.push(execution);
    }
}

struct MemoryStateSnapshot {
    run: Run,
    messages: Vec<Message>,
    attempts: Vec<RunAttempt>,
    tools: Vec<ToolExecution>,
    commits: Vec<String>,
}

#[async_trait]
impl RunStore for MemoryStore {
    async fn load_run(&self, id: &RunId) -> Result<Run, RunStoreError> {
        self.0
            .lock()
            .unwrap()
            .run
            .clone()
            .filter(|run| &run.id == id)
            .ok_or_else(|| RunStoreError::NotFound(id.clone()))
    }

    async fn load_message_path(
        &self,
        head: &MessageId,
    ) -> Result<Vec<ProjectedMessage>, RunStoreError> {
        let state = self.0.lock().unwrap();
        let mut path = Vec::new();
        let mut cursor = Some(*head);
        while let Some(id) = cursor {
            let message = state
                .messages
                .get(&id)
                .cloned()
                .ok_or_else(|| RunStoreError::Other(format!("message not found: {id}")))?;
            cursor = message.parent_message_id;
            path.push(ProjectedMessage::Visible(message));
        }
        path.reverse();
        Ok(path)
    }

    async fn load_attempts(&self, run_id: &RunId) -> Result<Vec<RunAttempt>, RunStoreError> {
        let mut attempts: Vec<_> = self
            .0
            .lock()
            .unwrap()
            .attempts
            .iter()
            .filter(|attempt| &attempt.run_id == run_id)
            .cloned()
            .collect();
        attempts.sort_by_key(|attempt| attempt.number);
        Ok(attempts)
    }

    async fn load_tool_executions(
        &self,
        run_id: &RunId,
        assistant_message_id: &MessageId,
    ) -> Result<Vec<ToolExecution>, RunStoreError> {
        let mut tools: Vec<_> = self
            .0
            .lock()
            .unwrap()
            .tools
            .iter()
            .filter(|tool| {
                &tool.run_id == run_id && &tool.assistant_message_id == assistant_message_id
            })
            .cloned()
            .collect();
        tools.sort_by_key(|tool| (tool.tool_use_index, tool.attempt));
        Ok(tools)
    }

    async fn save_run(&self, run: Run) -> Result<Run, RunStoreError> {
        run.validate()
            .map_err(|error| RunStoreError::Other(error.to_string()))?;
        let mut state = self.0.lock().unwrap();
        state.commits.push(format!("run:{:?}", run.status));
        state.run = Some(run.clone());
        Ok(run)
    }

    async fn save_attempt(&self, run: Run, attempt: RunAttempt) -> Result<Run, RunStoreError> {
        attempt
            .validate()
            .map_err(|error| RunStoreError::Other(error.to_string()))?;
        let mut state = self.0.lock().unwrap();
        if let Some(existing) = state
            .attempts
            .iter_mut()
            .find(|candidate| candidate.id == attempt.id)
        {
            *existing = attempt.clone();
        } else {
            state.attempts.push(attempt.clone());
        }
        state
            .commits
            .push(format!("attempt:{}:{:?}", attempt.number, attempt.status));
        state.run = Some(run.clone());
        Ok(run)
    }

    async fn append_message(&self, run: Run, message: Message) -> Result<Run, RunStoreError> {
        message
            .validate()
            .map_err(|error| RunStoreError::Other(error.to_string()))?;
        let mut state = self.0.lock().unwrap();
        state.commits.push(format!("message:{}", message.id));
        state.messages.insert(message.id, message);
        state.run = Some(run.clone());
        Ok(run)
    }

    async fn save_tool_execution(
        &self,
        run: Run,
        execution: ToolExecution,
    ) -> Result<Run, RunStoreError> {
        execution
            .validate()
            .map_err(|error| RunStoreError::Other(error.to_string()))?;
        let mut state = self.0.lock().unwrap();
        if let Some(existing) = state
            .tools
            .iter_mut()
            .find(|candidate| candidate.id == execution.id)
        {
            *existing = execution.clone();
        } else {
            state.tools.push(execution.clone());
        }
        state
            .commits
            .push(format!("tool:{}:{:?}", execution.call_id, execution.status));
        state.run = Some(run.clone());
        Ok(run)
    }

    async fn append_tool_result(
        &self,
        run: Run,
        execution: ToolExecution,
        message: Message,
    ) -> Result<Run, RunStoreError> {
        let mut unlinked = execution.clone();
        unlinked.tool_result_message_id = None;
        unlinked
            .validate_result_message(&message)
            .map_err(|error| RunStoreError::Other(error.to_string()))?;
        let mut state = self.0.lock().unwrap();
        let existing = state
            .tools
            .iter_mut()
            .find(|candidate| candidate.id == execution.id)
            .ok_or_else(|| RunStoreError::Other("tool intent missing".into()))?;
        *existing = execution.clone();
        state.commits.push(format!("result:{}", execution.call_id));
        state.messages.insert(message.id, message);
        state.run = Some(run.clone());
        Ok(run)
    }

    async fn try_complete(
        &self,
        run: Run,
        expected_queue_version: u64,
    ) -> Result<CompletionResult, RunStoreError> {
        let mut state = self.0.lock().unwrap();
        let current = state.run.as_ref().unwrap();
        if current.queue_version != expected_queue_version {
            return Ok(CompletionResult::QueueChanged(current.clone()));
        }
        run.validate()
            .map_err(|error| RunStoreError::Other(error.to_string()))?;
        state.commits.push("complete".into());
        state.run = Some(run.clone());
        Ok(CompletionResult::Completed(run))
    }

    async fn drain_queue(&self, mut run: Run) -> Result<Run, RunStoreError> {
        run.queue_cursor = run.queue_version;
        run.phase = RunPhase::AssemblingContext;
        self.save_run(run).await
    }
}

#[derive(Default)]
struct ScriptedAgent {
    responses: Mutex<VecDeque<Result<AgentResponse, DomainError>>>,
    paths: Mutex<Vec<Vec<ProjectedMessage>>>,
}

impl ScriptedAgent {
    fn new(responses: Vec<Result<AgentResponse, DomainError>>) -> Self {
        Self {
            responses: Mutex::new(responses.into()),
            paths: Mutex::new(Vec::new()),
        }
    }
}

#[async_trait]
impl RunAgent for ScriptedAgent {
    async fn invoke(&self, request: AgentInvocation) -> Result<AgentResponse, DomainError> {
        self.paths.lock().unwrap().push(request.message_path);
        self.responses
            .lock()
            .unwrap()
            .pop_front()
            .expect("scripted agent response")
    }
}

#[derive(Default)]
struct ScriptedTools {
    approval_names: BTreeSet<String>,
    outcomes: Mutex<BTreeMap<String, VecDeque<Result<ToolOutcome, DomainError>>>>,
    calls: Mutex<Vec<String>>,
}

impl ScriptedTools {
    fn with_outcomes(
        outcomes: impl IntoIterator<Item = (&'static str, Result<ToolOutcome, DomainError>)>,
    ) -> Self {
        let mut grouped: BTreeMap<String, VecDeque<_>> = BTreeMap::new();
        for (name, outcome) in outcomes {
            grouped.entry(name.into()).or_default().push_back(outcome);
        }
        Self {
            outcomes: Mutex::new(grouped),
            ..Self::default()
        }
    }
}

#[async_trait]
impl RunTool for ScriptedTools {
    fn requires_approval(&self, tool_name: &str, _arguments: &serde_json::Value) -> bool {
        self.approval_names.contains(tool_name)
    }

    async fn execute(&self, request: ToolInvocation) -> Result<ToolOutcome, DomainError> {
        self.calls.lock().unwrap().push(request.call_id);
        self.outcomes
            .lock()
            .unwrap()
            .get_mut(&request.tool_name)
            .and_then(VecDeque::pop_front)
            .unwrap_or_else(|| {
                Ok(ToolOutcome {
                    output: json!(null),
                })
            })
    }

    async fn reconcile(&self, _execution: &ToolExecution) -> Result<ToolRecovery, DomainError> {
        Ok(ToolRecovery::Unknown)
    }
}

struct NeverCompletesTool;

#[async_trait]
impl RunTool for NeverCompletesTool {
    fn requires_approval(&self, _tool_name: &str, _arguments: &serde_json::Value) -> bool {
        false
    }

    async fn execute(&self, _request: ToolInvocation) -> Result<ToolOutcome, DomainError> {
        std::future::pending().await
    }

    async fn reconcile(&self, _execution: &ToolExecution) -> Result<ToolRecovery, DomainError> {
        Ok(ToolRecovery::Unknown)
    }
}

struct ScriptedApprovals(Mutex<VecDeque<ApprovalDecision>>);

impl ScriptedApprovals {
    fn new(decisions: Vec<ApprovalDecision>) -> Self {
        Self(Mutex::new(decisions.into()))
    }
}

#[async_trait]
impl RunApproval for ScriptedApprovals {
    async fn decide(&self, _request: ApprovalRequest) -> Result<ApprovalDecision, DomainError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(ApprovalDecision::Approved))
    }
}

#[derive(Default)]
struct ManualClock(AtomicI64);

impl ManualClock {
    fn at(value: i64) -> Self {
        Self(AtomicI64::new(value))
    }
}

#[async_trait]
impl RunClock for ManualClock {
    fn now(&self) -> TimestampMs {
        TimestampMs(self.0.load(Ordering::SeqCst))
    }

    async fn sleep_until(&self, deadline: TimestampMs) {
        self.0.store(deadline.0, Ordering::SeqCst);
    }
}

#[derive(Default)]
struct SequenceIds(AtomicU64);

impl RunIdGenerator for SequenceIds {
    fn message_id(&self) -> MessageId {
        MessageId::from_u128(u128::from(self.0.fetch_add(1, Ordering::SeqCst) + 100))
    }

    fn attempt_id(&self) -> RunAttemptId {
        RunAttemptId::new(format!("attempt-{}", self.0.fetch_add(1, Ordering::SeqCst)))
    }

    fn tool_execution_id(&self) -> ToolExecutionId {
        ToolExecutionId::new(format!("tool-{}", self.0.fetch_add(1, Ordering::SeqCst)))
    }
}

fn snapshot() -> AgentConfigSnapshot {
    AgentConfigSnapshot {
        agent_id: AgentId::new("agent-1"),
        revision: 1,
        driver_type: "mock".into(),
        connection_name: "test".into(),
        model: "test".into(),
        endpoint: None,
        capabilities: BTreeSet::from([AgentCapability::Text, AgentCapability::ToolUse]),
        default_parameters: DomainMetadata::default(),
        tool_policy: ToolPolicy::default(),
        config_digest: "a".repeat(64),
    }
}

fn fixture() -> (Run, Vec<Message>) {
    let root = Message {
        id: MessageId::from_u128(1),
        project_id: ProjectId::new("project-1"),
        parent_message_id: None,
        role: MessageRole::System,
        kind: MessageKind::Standard,
        origin: MessageOrigin::Project,
        sub_messages: vec![SubMessage::Text {
            text: "instructions".into(),
        }],
        created_by_session_id: None,
        run_id: None,
        run_seq: None,
        tool_result: None,
        metadata: DomainMetadata::default(),
        created_at: TimestampMs(1),
    };
    let user = Message {
        id: MessageId::from_u128(2),
        project_id: ProjectId::new("project-1"),
        parent_message_id: Some(root.id),
        role: MessageRole::User,
        kind: MessageKind::Standard,
        origin: MessageOrigin::Human,
        sub_messages: vec![SubMessage::Text { text: "go".into() }],
        created_by_session_id: None,
        run_id: None,
        run_seq: None,
        tool_result: None,
        metadata: DomainMetadata::default(),
        created_at: TimestampMs(2),
    };
    let run = Run {
        id: RunId::new("run-1"),
        project_id: ProjectId::new("project-1"),
        base_message_id: user.id,
        last_message_id: None,
        follow_session_id: None,
        agent_id: AgentId::new("agent-1"),
        agent_revision: 1,
        agent_snapshot: snapshot(),
        trigger: RunTrigger::Manual,
        cron_id: None,
        scheduled_at: None,
        status: RunStatus::Queued,
        phase: RunPhase::Queued,
        stop_reason: None,
        error: None,
        step_count: 0,
        budget: RunBudget {
            max_steps: 20,
            token_budget: None,
            cost_budget: None,
            max_runtime: None,
        },
        usage: RunUsage::default(),
        attempt_count: 0,
        compaction_count: 0,
        retry_policy: RetryPolicy {
            max_attempts: 3,
            initial_delay: DurationMs(10),
            max_delay: DurationMs(100),
        },
        next_retry_at: None,
        checkpoint_id: None,
        queue_version: 0,
        queue_cursor: 0,
        dedupe_key: None,
        started_at: None,
        ended_at: None,
        created_at: TimestampMs(1),
    };
    (run, vec![root, user])
}

fn text(text: &str) -> AgentResponse {
    AgentResponse {
        sub_messages: vec![SubMessage::Text { text: text.into() }],
        usage: RunUsage {
            input_tokens: 2,
            output_tokens: 3,
            ..RunUsage::default()
        },
    }
}

fn tool_calls(calls: &[(&str, &str)]) -> AgentResponse {
    AgentResponse {
        sub_messages: calls
            .iter()
            .map(|(call_id, name)| {
                SubMessage::ToolUse(ToolUse {
                    call_id: (*call_id).into(),
                    tool_name: (*name).into(),
                    arguments: "{}".into(),
                    provider_metadata: None,
                })
            })
            .collect(),
        usage: RunUsage::default(),
    }
}

fn coordinator(
    store: Arc<MemoryStore>,
    agent: Arc<ScriptedAgent>,
    tools: Arc<ScriptedTools>,
    approvals: Arc<ScriptedApprovals>,
    clock: Arc<ManualClock>,
) -> RunCoordinator {
    RunCoordinator::new(
        store,
        agent,
        tools,
        approvals,
        clock,
        Arc::new(SequenceIds::default()),
    )
}

#[tokio::test]
async fn completes_an_ordinary_reply_after_durable_message_commit() {
    let (run, messages) = fixture();
    let store = Arc::new(MemoryStore::seeded(run, messages));
    let agent = Arc::new(ScriptedAgent::new(vec![Ok(text("done"))]));
    let engine = coordinator(
        store.clone(),
        agent,
        Arc::new(ScriptedTools::default()),
        Arc::new(ScriptedApprovals::new(vec![])),
        Arc::new(ManualClock::at(10)),
    );

    let outcome = engine
        .drive(&RunId::new("run-1"), CancellationToken::new())
        .await
        .unwrap();

    assert!(matches!(outcome, DriveOutcome::Completed(_)));
    let snapshot = store.snapshot();
    assert_eq!(snapshot.run.status, RunStatus::Completed);
    assert_eq!(snapshot.run.step_count, 1);
    let message_commit = snapshot
        .commits
        .iter()
        .position(|entry| entry.starts_with("message:"))
        .unwrap();
    let completion = snapshot
        .commits
        .iter()
        .position(|entry| entry == "complete")
        .unwrap();
    assert!(message_commit < completion);
}

#[tokio::test]
async fn executes_multiple_tools_and_appends_results_in_tool_use_order() {
    let (run, messages) = fixture();
    let store = Arc::new(MemoryStore::seeded(run, messages));
    let agent = Arc::new(ScriptedAgent::new(vec![
        Ok(tool_calls(&[("call-b", "second"), ("call-a", "first")])),
        Ok(text("finished")),
    ]));
    let tools = Arc::new(ScriptedTools::with_outcomes([
        ("second", Ok(ToolOutcome { output: json!(2) })),
        ("first", Ok(ToolOutcome { output: json!(1) })),
    ]));
    let engine = coordinator(
        store.clone(),
        agent.clone(),
        tools.clone(),
        Arc::new(ScriptedApprovals::new(vec![])),
        Arc::new(ManualClock::at(10)),
    );

    let outcome = engine
        .drive(&RunId::new("run-1"), CancellationToken::new())
        .await
        .unwrap();

    assert!(matches!(outcome, DriveOutcome::Completed(_)));
    assert_eq!(*tools.calls.lock().unwrap(), ["call-b", "call-a"]);
    let paths = agent.paths.lock().unwrap();
    let second_path = &paths[1];
    let result_ids: Vec<_> = second_path
        .iter()
        .filter_map(|message| match message {
            ProjectedMessage::Visible(Message {
                tool_result: Some(result),
                ..
            }) => Some(result.call_id.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(result_ids, ["call-b", "call-a"]);
}

#[tokio::test]
async fn tool_failure_becomes_a_failed_result_and_the_agent_continues() {
    let (run, messages) = fixture();
    let store = Arc::new(MemoryStore::seeded(run, messages));
    let agent = Arc::new(ScriptedAgent::new(vec![
        Ok(tool_calls(&[("call-1", "broken")])),
        Ok(text("handled")),
    ]));
    let tools = Arc::new(ScriptedTools::with_outcomes([(
        "broken",
        Err(DomainError::invariant(
            ErrorCode::ToolExecutionFailed,
            "boom",
        )),
    )]));
    let engine = coordinator(
        store.clone(),
        agent,
        tools,
        Arc::new(ScriptedApprovals::new(vec![])),
        Arc::new(ManualClock::at(10)),
    );

    engine
        .drive(&RunId::new("run-1"), CancellationToken::new())
        .await
        .unwrap();

    let snapshot = store.snapshot();
    let result = snapshot
        .messages
        .iter()
        .find_map(|message| message.tool_result.as_ref())
        .unwrap();
    assert_eq!(result.status, ToolResultStatus::Failed);
    assert!(result.error.as_deref().unwrap().contains("boom"));
    assert_eq!(snapshot.run.status, RunStatus::Completed);
}

#[tokio::test]
async fn retries_a_transient_agent_failure_inside_the_same_run() {
    let (run, messages) = fixture();
    let store = Arc::new(MemoryStore::seeded(run, messages));
    let agent = Arc::new(ScriptedAgent::new(vec![
        Err(DomainError::transient(
            ErrorCode::RunRecoveryFailed,
            "temporary",
        )),
        Ok(text("recovered")),
    ]));
    let engine = coordinator(
        store.clone(),
        agent,
        Arc::new(ScriptedTools::default()),
        Arc::new(ScriptedApprovals::new(vec![])),
        Arc::new(ManualClock::at(10)),
    );

    engine
        .drive(&RunId::new("run-1"), CancellationToken::new())
        .await
        .unwrap();

    let snapshot = store.snapshot();
    assert_eq!(snapshot.run.status, RunStatus::Completed);
    assert_eq!(snapshot.run.attempt_count, 2);
    assert_eq!(snapshot.attempts.len(), 2);
    assert!(
        snapshot
            .commits
            .iter()
            .any(|entry| entry == "run:RetryWait")
    );
}

#[tokio::test]
async fn waits_for_approval_then_resumes_without_a_second_tool_intent() {
    let (run, messages) = fixture();
    let store = Arc::new(MemoryStore::seeded(run, messages));
    let agent = Arc::new(ScriptedAgent::new(vec![
        Ok(tool_calls(&[("call-1", "guarded")])),
        Ok(text("done")),
    ]));
    let mut tool_adapter = ScriptedTools::with_outcomes([(
        "guarded",
        Ok(ToolOutcome {
            output: json!("ok"),
        }),
    )]);
    tool_adapter.approval_names.insert("guarded".into());
    let tools = Arc::new(tool_adapter);
    let approvals = Arc::new(ScriptedApprovals::new(vec![
        ApprovalDecision::Pending,
        ApprovalDecision::Approved,
    ]));
    let engine = coordinator(
        store.clone(),
        agent,
        tools,
        approvals,
        Arc::new(ManualClock::at(10)),
    );

    let first = engine
        .drive(&RunId::new("run-1"), CancellationToken::new())
        .await
        .unwrap();
    assert!(matches!(first, DriveOutcome::WaitingApproval(_)));
    let second = engine
        .drive(&RunId::new("run-1"), CancellationToken::new())
        .await
        .unwrap();
    assert!(matches!(second, DriveOutcome::Completed(_)));
    assert_eq!(store.snapshot().tools.len(), 1);
}

#[tokio::test]
async fn cancellation_and_all_configured_budgets_force_terminal_states() {
    let (run, messages) = fixture();
    let store = Arc::new(MemoryStore::seeded(run, messages));
    let engine = coordinator(
        store.clone(),
        Arc::new(ScriptedAgent::new(vec![])),
        Arc::new(ScriptedTools::default()),
        Arc::new(ScriptedApprovals::new(vec![])),
        Arc::new(ManualClock::at(10)),
    );
    let cancellation = CancellationToken::new();
    cancellation.cancel();
    let outcome = engine
        .drive(&RunId::new("run-1"), cancellation)
        .await
        .unwrap();
    assert!(matches!(outcome, DriveOutcome::Terminal(_)));
    assert_eq!(store.snapshot().run.status, RunStatus::Cancelled);

    for (token_budget, cost_budget, usage, expected) in [
        (
            Some(5),
            None,
            RunUsage {
                input_tokens: 2,
                output_tokens: 3,
                ..RunUsage::default()
            },
            RunStopReason::TokenBudget,
        ),
        (
            None,
            Some(CostMicros(7)),
            RunUsage {
                cost: Some(CostMicros(7)),
                ..RunUsage::default()
            },
            RunStopReason::CostBudget,
        ),
    ] {
        let (mut run, messages) = fixture();
        run.budget.token_budget = token_budget;
        run.budget.cost_budget = cost_budget;
        let store = Arc::new(MemoryStore::seeded(run, messages));
        let engine = coordinator(
            store.clone(),
            Arc::new(ScriptedAgent::new(vec![Ok(AgentResponse {
                sub_messages: text("done").sub_messages,
                usage,
            })])),
            Arc::new(ScriptedTools::default()),
            Arc::new(ScriptedApprovals::new(vec![])),
            Arc::new(ManualClock::at(10)),
        );
        engine
            .drive(&RunId::new("run-1"), CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(store.snapshot().run.stop_reason, Some(expected));
    }

    let (mut run, messages) = fixture();
    run.status = RunStatus::Running;
    run.phase = RunPhase::AssemblingContext;
    run.started_at = Some(TimestampMs(10));
    run.budget.max_runtime = Some(DurationMs(5));
    let store = Arc::new(MemoryStore::seeded(run, messages));
    let engine = coordinator(
        store.clone(),
        Arc::new(ScriptedAgent::new(vec![])),
        Arc::new(ScriptedTools::default()),
        Arc::new(ScriptedApprovals::new(vec![])),
        Arc::new(ManualClock::at(15)),
    );
    engine
        .drive(&RunId::new("run-1"), CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(
        store.snapshot().run.stop_reason,
        Some(RunStopReason::RuntimeLimit)
    );

    let (mut run, messages) = fixture();
    run.budget.max_steps = 1;
    let store = Arc::new(MemoryStore::seeded(run, messages));
    let engine = coordinator(
        store.clone(),
        Arc::new(ScriptedAgent::new(vec![Ok(tool_calls(&[(
            "call-1",
            "never_run",
        )]))])),
        Arc::new(ScriptedTools::default()),
        Arc::new(ScriptedApprovals::new(vec![])),
        Arc::new(ManualClock::at(10)),
    );
    engine
        .drive(&RunId::new("run-1"), CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(
        store.snapshot().run.stop_reason,
        Some(RunStopReason::StepLimit)
    );
}

#[tokio::test]
async fn a_hung_tool_is_cancelled_at_the_persisted_runtime_deadline() {
    let (mut run, messages) = fixture();
    run.budget.max_runtime = Some(DurationMs(5));
    let store = Arc::new(MemoryStore::seeded(run, messages));
    let engine = RunCoordinator::new(
        store.clone(),
        Arc::new(ScriptedAgent::new(vec![Ok(tool_calls(&[(
            "call-timeout",
            "hang",
        )]))])),
        Arc::new(NeverCompletesTool),
        Arc::new(ScriptedApprovals::new(vec![])),
        Arc::new(ManualClock::at(10)),
        Arc::new(SequenceIds::default()),
    );

    let outcome = engine
        .drive(&RunId::new("run-1"), CancellationToken::new())
        .await
        .unwrap();

    assert!(matches!(outcome, DriveOutcome::Terminal(_)));
    let snapshot = store.snapshot();
    assert_eq!(snapshot.run.status, RunStatus::LimitExceeded);
    assert_eq!(snapshot.run.stop_reason, Some(RunStopReason::RuntimeLimit));
    let execution = snapshot.tools.last().unwrap();
    assert_eq!(execution.status, ToolExecutionStatus::Cancelled);
    assert!(execution.tool_result_message_id.is_some());
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn crash_recovery_persists_a_known_tool_outcome_without_reexecution() {
    let (mut run, mut messages) = fixture();
    let assistant = Message {
        id: MessageId::from_u128(3),
        project_id: run.project_id.clone(),
        parent_message_id: Some(run.base_message_id),
        role: MessageRole::Assistant,
        kind: MessageKind::Standard,
        origin: MessageOrigin::Agent,
        sub_messages: tool_calls(&[("call-1", "side_effect"), ("call-2", "side_effect")])
            .sub_messages,
        created_by_session_id: None,
        run_id: Some(run.id.clone()),
        run_seq: Some(1),
        tool_result: None,
        metadata: DomainMetadata::default(),
        created_at: TimestampMs(11),
    };
    run.status = RunStatus::Running;
    run.phase = RunPhase::PersistingToolResult;
    run.started_at = Some(TimestampMs(10));
    let first_result = Message {
        id: MessageId::from_u128(4),
        project_id: run.project_id.clone(),
        parent_message_id: Some(assistant.id),
        role: MessageRole::User,
        kind: MessageKind::ToolResult,
        origin: MessageOrigin::Tool,
        sub_messages: Vec::new(),
        created_by_session_id: None,
        run_id: Some(run.id.clone()),
        run_seq: Some(2),
        tool_result: Some(ait_domain::ToolResult {
            call_id: "call-1".into(),
            status: ToolResultStatus::Succeeded,
            output: Some("{\"already\":\"done\"}".into()),
            error: None,
        }),
        metadata: DomainMetadata::default(),
        created_at: TimestampMs(14),
    };
    run.last_message_id = Some(first_result.id);
    run.step_count = 2;
    run.attempt_count = 1;
    messages.push(assistant.clone());
    messages.push(first_result.clone());
    let store = Arc::new(MemoryStore::seeded(run.clone(), messages));
    store.insert_tool(ToolExecution {
        id: ToolExecutionId::new("tool-existing"),
        run_id: run.id.clone(),
        call_id: "call-1".into(),
        assistant_message_id: assistant.id,
        tool_use_index: 0,
        tool_result_message_id: Some(first_result.id),
        tool_name: "side_effect".into(),
        arguments: json!({}),
        attempt: 1,
        approval_status: ToolApprovalStatus::NotRequired,
        status: ToolExecutionStatus::Succeeded,
        result: Some(json!({"already": "done"})),
        error: None,
        started_at: Some(TimestampMs(12)),
        ended_at: Some(TimestampMs(13)),
        created_at: TimestampMs(12),
    });
    store.insert_tool(ToolExecution {
        id: ToolExecutionId::new("tool-second"),
        run_id: run.id.clone(),
        call_id: "call-2".into(),
        assistant_message_id: assistant.id,
        tool_use_index: 1,
        tool_result_message_id: None,
        tool_name: "side_effect".into(),
        arguments: json!({}),
        attempt: 1,
        approval_status: ToolApprovalStatus::NotRequired,
        status: ToolExecutionStatus::Succeeded,
        result: Some(json!({"also": "done"})),
        error: None,
        started_at: Some(TimestampMs(12)),
        ended_at: Some(TimestampMs(13)),
        created_at: TimestampMs(12),
    });
    let tools = Arc::new(ScriptedTools::default());
    let engine = coordinator(
        store.clone(),
        Arc::new(ScriptedAgent::new(vec![Ok(text("final"))])),
        tools.clone(),
        Arc::new(ScriptedApprovals::new(vec![])),
        Arc::new(ManualClock::at(20)),
    );

    engine
        .drive(&RunId::new("run-1"), CancellationToken::new())
        .await
        .unwrap();

    assert!(tools.calls.lock().unwrap().is_empty());
    let snapshot = store.snapshot();
    assert_eq!(snapshot.run.status, RunStatus::Completed);
    assert_eq!(snapshot.tools.len(), 2);
    assert!(
        snapshot
            .tools
            .iter()
            .all(|execution| execution.tool_result_message_id.is_some())
    );
}
