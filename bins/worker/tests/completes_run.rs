//! Worker-level vertical slice for one complete no-tool Run.

use std::{
    collections::{BTreeSet, HashMap},
    sync::{Arc, Mutex},
};

use ait_domain::{
    AgentCapability, AgentConfigSnapshot, AgentId, DomainError, DomainMetadata, DurationMs,
    GitCommit, Message, MessageId, MessageKind, MessageOrigin, MessageRole, ProjectId,
    ProjectedMessage, RetryPolicy, Run, RunAttempt, RunAttemptStatus, RunBudget, RunId, RunPhase,
    RunStatus, RunStopReason, RunTrigger, RunUsage, SubMessage, TimestampMs, ToolExecution,
    ToolPolicy,
};
use ait_ports::{
    AgentInvocation, AgentResponse, ApprovalDecision, ApprovalRequest, CompletionResult, RunAgent,
    RunApproval, RunStore, RunStoreError, RunTool, ToolInvocation, ToolOutcome, ToolRecovery,
};
use ait_runtime::DriveOutcome;
use ait_worker::RunWorker;
use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

struct State {
    run: Run,
    messages: HashMap<MessageId, Message>,
    attempts: Vec<RunAttempt>,
}

struct DaemonStore(Mutex<State>);

impl DaemonStore {
    fn seeded(run: Run, messages: Vec<Message>) -> Self {
        Self(Mutex::new(State {
            run,
            messages: messages
                .into_iter()
                .map(|message| (message.id, message))
                .collect(),
            attempts: Vec::new(),
        }))
    }

    fn snapshot(&self) -> State {
        let state = self.0.lock().unwrap();
        State {
            run: state.run.clone(),
            messages: state.messages.clone(),
            attempts: state.attempts.clone(),
        }
    }
}

#[async_trait]
impl RunStore for DaemonStore {
    async fn load_run(&self, id: &RunId) -> Result<Run, RunStoreError> {
        let state = self.0.lock().unwrap();
        if &state.run.id == id {
            Ok(state.run.clone())
        } else {
            Err(RunStoreError::NotFound(id.clone()))
        }
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
        let state = self.0.lock().unwrap();
        Ok(state
            .attempts
            .iter()
            .filter(|attempt| &attempt.run_id == run_id)
            .cloned()
            .collect())
    }

    async fn load_tool_executions(
        &self,
        _run_id: &RunId,
        _assistant_message_id: &MessageId,
    ) -> Result<Vec<ToolExecution>, RunStoreError> {
        Ok(Vec::new())
    }

    async fn save_run(&self, run: Run) -> Result<Run, RunStoreError> {
        run.validate()
            .map_err(|error| RunStoreError::Other(error.to_string()))?;
        self.0.lock().unwrap().run = run.clone();
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
            .find(|existing| existing.id == attempt.id)
        {
            *existing = attempt;
        } else {
            state.attempts.push(attempt);
        }
        state.run = run.clone();
        Ok(run)
    }

    async fn append_message(&self, run: Run, message: Message) -> Result<Run, RunStoreError> {
        message
            .validate()
            .map_err(|error| RunStoreError::Other(error.to_string()))?;
        let mut state = self.0.lock().unwrap();
        state.messages.insert(message.id, message);
        state.run = run.clone();
        Ok(run)
    }

    async fn save_tool_execution(
        &self,
        _run: Run,
        _execution: ToolExecution,
    ) -> Result<Run, RunStoreError> {
        Err(RunStoreError::Other(
            "a no-tool Run must not persist tool execution".into(),
        ))
    }

    async fn append_tool_result(
        &self,
        _run: Run,
        _execution: ToolExecution,
        _message: Message,
    ) -> Result<Run, RunStoreError> {
        Err(RunStoreError::Other(
            "a no-tool Run must not append a tool result".into(),
        ))
    }

    async fn try_complete(
        &self,
        run: Run,
        expected_queue_version: u64,
    ) -> Result<CompletionResult, RunStoreError> {
        run.validate()
            .map_err(|error| RunStoreError::Other(error.to_string()))?;
        let mut state = self.0.lock().unwrap();
        if state.run.queue_version != expected_queue_version {
            return Ok(CompletionResult::QueueChanged(state.run.clone()));
        }
        state.run = run.clone();
        Ok(CompletionResult::Completed(run))
    }

    async fn drain_queue(&self, run: Run) -> Result<Run, RunStoreError> {
        self.save_run(run).await
    }
}

struct OneReplyAgent;

#[async_trait]
impl RunAgent for OneReplyAgent {
    async fn invoke(&self, request: AgentInvocation) -> Result<AgentResponse, DomainError> {
        assert_eq!(request.run_id, RunId::new("run-worker-test"));
        assert_eq!(request.message_path.len(), 2);
        Ok(AgentResponse {
            sub_messages: vec![SubMessage::Text {
                text: "worker completed the run".into(),
            }],
            usage: RunUsage {
                input_tokens: 2,
                output_tokens: 4,
                ..RunUsage::default()
            },
        })
    }
}

struct NoTools;

#[async_trait]
impl RunTool for NoTools {
    fn requires_approval(&self, _tool_name: &str, _arguments: &serde_json::Value) -> bool {
        false
    }

    async fn execute(&self, _request: ToolInvocation) -> Result<ToolOutcome, DomainError> {
        Err(DomainError::invariant(
            ait_domain::ErrorCode::ToolExecutionFailed,
            "a no-tool Run must not execute a tool",
        ))
    }

    async fn reconcile(&self, _execution: &ToolExecution) -> Result<ToolRecovery, DomainError> {
        Ok(ToolRecovery::Unknown)
    }
}

struct ApproveAll;

#[async_trait]
impl RunApproval for ApproveAll {
    async fn decide(&self, _request: ApprovalRequest) -> Result<ApprovalDecision, DomainError> {
        Ok(ApprovalDecision::Approved)
    }
}

fn fixture() -> (Run, Vec<Message>) {
    let project_id = ProjectId::new("project-worker-test");
    let root = Message {
        id: MessageId::from_u128(1),
        project_id: project_id.clone(),
        parent_message_id: None,
        role: MessageRole::System,
        kind: MessageKind::Standard,
        origin: MessageOrigin::Project,
        sub_messages: vec![SubMessage::Text {
            text: "test instructions".into(),
        }],
        created_by_session_id: None,
        run_id: None,
        run_seq: None,
        tool_result: None,
        git_commit: None,
        metadata: DomainMetadata::default(),
        created_at: TimestampMs(1),
    };
    let user = Message {
        id: MessageId::from_u128(2),
        project_id: project_id.clone(),
        parent_message_id: Some(root.id),
        role: MessageRole::User,
        kind: MessageKind::Standard,
        origin: MessageOrigin::Human,
        sub_messages: vec![SubMessage::Text {
            text: "complete this run".into(),
        }],
        created_by_session_id: None,
        run_id: None,
        run_seq: None,
        tool_result: None,
        git_commit: Some(GitCommit::parse("a".repeat(40)).unwrap()),
        metadata: DomainMetadata::default(),
        created_at: TimestampMs(2),
    };
    let agent_id = AgentId::new("agent-worker-test");
    let run = Run {
        id: RunId::new("run-worker-test"),
        project_id,
        base_message_id: user.id,
        last_message_id: None,
        follow_session_id: None,
        agent_id: agent_id.clone(),
        agent_revision: 1,
        agent_snapshot: AgentConfigSnapshot {
            agent_id,
            revision: 1,
            driver_type: "test".into(),
            connection_name: "test".into(),
            model: "deterministic".into(),
            endpoint: None,
            capabilities: BTreeSet::from([AgentCapability::Text]),
            default_parameters: DomainMetadata::default(),
            tool_policy: ToolPolicy::default(),
            config_digest: "a".repeat(64),
        },
        trigger: RunTrigger::Manual,
        cron_id: None,
        scheduled_at: None,
        status: RunStatus::Queued,
        phase: RunPhase::Queued,
        stop_reason: None,
        error: None,
        step_count: 0,
        budget: RunBudget {
            max_steps: 4,
            token_budget: None,
            cost_budget: None,
            max_runtime: None,
        },
        usage: RunUsage::default(),
        attempt_count: 0,
        compaction_count: 0,
        retry_policy: RetryPolicy {
            max_attempts: 1,
            initial_delay: DurationMs(0),
            max_delay: DurationMs(0),
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

#[tokio::test]
async fn worker_completes_one_run() {
    let (run, messages) = fixture();
    let store = Arc::new(DaemonStore::seeded(run, messages));
    let worker = RunWorker::new(
        store.clone(),
        Arc::new(OneReplyAgent),
        Arc::new(NoTools),
        Arc::new(ApproveAll),
    );

    let outcome = worker
        .execute(&RunId::new("run-worker-test"), CancellationToken::new())
        .await
        .unwrap();

    assert!(matches!(outcome, DriveOutcome::Completed(_)));
    let state = store.snapshot();
    assert_eq!(state.run.status, RunStatus::Completed);
    assert_eq!(state.run.stop_reason, Some(RunStopReason::Completed));
    assert_eq!(state.run.step_count, 1);
    assert_eq!(state.run.attempt_count, 1);
    assert_eq!(state.attempts.len(), 1);
    assert_eq!(state.attempts[0].status, RunAttemptStatus::Completed);
    assert_eq!(state.run.usage.input_tokens, 2);
    assert_eq!(state.run.usage.output_tokens, 4);

    let output = state
        .messages
        .values()
        .find(|message| message.run_id.as_ref() == Some(&state.run.id))
        .expect("worker output must be committed");
    assert_eq!(output.parent_message_id, Some(state.run.base_message_id));
    assert_eq!(output.role, MessageRole::Assistant);
    assert_eq!(output.run_seq, Some(1));
    assert_eq!(
        output.sub_messages,
        vec![SubMessage::Text {
            text: "worker completed the run".into()
        }]
    );
}
