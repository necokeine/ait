use std::{path::PathBuf, sync::Arc};

use ait_domain::{
    DomainError, Message, MessageId, ProjectedMessage, Run, RunAttempt, RunAttemptId, RunId,
    RunUsage, TimestampMs, ToolExecution, ToolExecutionId,
};
use async_trait::async_trait;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

/// Stable failure exposed by Run persistence.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RunStoreError {
    /// The requested Run does not exist.
    NotFound(RunId),
    /// An optimistic state or queue-version check failed.
    Conflict(String),
    /// Adapter-specific failure with a safe diagnostic.
    Other(String),
}

impl std::fmt::Display for RunStoreError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound(id) => write!(formatter, "run not found: {}", id.as_str()),
            Self::Conflict(message) | Self::Other(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for RunStoreError {}

/// Result of the atomic Run termination barrier.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CompletionResult {
    /// The Run and optional followed Session were committed terminally.
    Completed(Run),
    /// Work arrived after the caller observed an empty queue.
    QueueChanged(Run),
}

/// Durable boundary used by the Run coordinator.
///
/// Methods accepting both a Run and a child record must persist them in one
/// transaction. A terminal Run write must also release its matching Session
/// `active_run_id`. Implementations use the Run counters/head as optimistic
/// preconditions and return [`RunStoreError::Conflict`] for stale writes.
#[async_trait]
pub trait RunStore: Send + Sync {
    /// Loads the latest Run snapshot.
    async fn load_run(&self, id: &RunId) -> Result<Run, RunStoreError>;

    /// Loads the immutable root-to-head path ending at `head`.
    async fn load_message_path(
        &self,
        head: &MessageId,
    ) -> Result<Vec<ProjectedMessage>, RunStoreError>;

    /// Loads attempts in ascending Run-local number order.
    async fn load_attempts(&self, run_id: &RunId) -> Result<Vec<RunAttempt>, RunStoreError>;

    /// Loads tool attempts for an assistant Message in stable tool-use order.
    async fn load_tool_executions(
        &self,
        run_id: &RunId,
        assistant_message_id: &MessageId,
    ) -> Result<Vec<ToolExecution>, RunStoreError>;

    /// Persists a Run state transition before the coordinator continues.
    async fn save_run(&self, run: Run) -> Result<Run, RunStoreError>;

    /// Atomically persists a Run transition and its attempt record.
    async fn save_attempt(&self, run: Run, attempt: RunAttempt) -> Result<Run, RunStoreError>;

    /// Atomically appends an immutable Message, updates Run head/sequence and
    /// compare-and-swaps the followed Session pointer when one exists.
    async fn append_message(&self, run: Run, message: Message) -> Result<Run, RunStoreError>;

    /// Atomically persists a Run transition and tool execution state.
    async fn save_tool_execution(
        &self,
        run: Run,
        execution: ToolExecution,
    ) -> Result<Run, RunStoreError>;

    /// Atomically appends the unique `ToolResult` Message, links it to the
    /// terminal execution and advances the Run/Session head.
    async fn append_tool_result(
        &self,
        run: Run,
        execution: ToolExecution,
        message: Message,
    ) -> Result<Run, RunStoreError>;

    /// Atomically completes only if all durable blockers are clear and the
    /// queue version still equals `expected_queue_version`.
    async fn try_complete(
        &self,
        run: Run,
        expected_queue_version: u64,
    ) -> Result<CompletionResult, RunStoreError>;

    /// Persists queued inputs and returns the Run after all currently visible
    /// work has been consumed. The next model turn is assembled from its head.
    async fn drain_queue(&self, run: Run) -> Result<Run, RunStoreError>;
}

/// One complete, normalized Agent turn request.
#[derive(Clone, Debug, PartialEq)]
pub struct AgentInvocation {
    /// Idempotency/correlation identity for the low-level attempt.
    pub attempt_id: RunAttemptId,
    /// Fixed Run identity.
    pub run_id: RunId,
    /// Fixed Agent revision.
    pub agent_revision: u64,
    /// Ordered root-to-head Message path.
    pub message_path: Vec<ProjectedMessage>,
    /// Cooperative cancellation shared with the Run supervisor.
    pub cancellation: CancellationToken,
}

/// Complete Agent output accepted as one immutable assistant Message.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentResponse {
    /// Ordered assistant content, including zero or more `ToolUse` parts.
    pub sub_messages: Vec<ait_domain::SubMessage>,
    /// Usage charged by this Agent turn.
    pub usage: RunUsage,
}

/// Agent/Provider adapter boundary consumed by the coordinator.
#[async_trait]
pub trait RunAgent: Send + Sync {
    /// Executes one model turn over the supplied path.
    async fn invoke(&self, request: AgentInvocation) -> Result<AgentResponse, DomainError>;
}

/// Linearization boundary between cancellation and externally visible workspace integration.
#[async_trait]
pub trait WorkspaceIntegrationGate: std::fmt::Debug + Send + Sync {
    /// Claims finalization for the running invocation.
    ///
    /// Once this succeeds, cancellation must not persist a cancelled terminal
    /// state. If cancellation won first, this returns [`ErrorCode::RunCancelled`].
    async fn begin_integration(&self) -> Result<(), DomainError>;

    /// Observes a named integration boundary after finalization was claimed.
    ///
    /// Production gates normally keep the default no-op implementation. The
    /// explicit checkpoints make failure/race injection deterministic without
    /// teaching an adapter about a concrete persistence or test implementation.
    async fn checkpoint(
        &self,
        _checkpoint: WorkspaceIntegrationCheckpoint,
    ) -> Result<(), DomainError> {
        Ok(())
    }
}

/// Fallible boundaries in the primary-worktree publication protocol.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorkspaceIntegrationCheckpoint {
    /// The ref transaction is prepared, before the canonical index is locked.
    BeforeIndexLock,
    /// The primary worktree is still at the admitted tree, before updating it.
    BeforeWorktreeUpdate,
    /// Collision scanning is complete, immediately before paths are quarantined.
    BeforeWorktreeMutation,
    /// The primary worktree was updated through the locked candidate index.
    AfterWorktreeUpdate,
    /// All pre-publication validation passed, immediately before ref publication.
    BeforeRefPublish,
    /// Git applied the ref transaction, before its confirmation is accepted.
    BeforeRefCommitConfirmation,
    /// The target ref was published while the canonical index remains unchanged.
    AfterRefPublish,
    /// A baseline ref was observed, before its exact target lock is acquired.
    BeforeBaselineRefReconciliationLock,
    /// The candidate view is fixed, immediately before rollback isolates live paths.
    BeforeWorktreeRollback,
    /// A rollback candidate was isolated and verified, before restoring the baseline.
    AfterRollbackCandidateQuarantine,
    /// The canonical index is about to become the candidate index.
    BeforeIndexPublish,
}

/// One workspace-scoped invocation of a complete coding Agent harness.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AdoptedWorkspace {
    /// Retained manager-owned worktree to continue.
    pub path: PathBuf,
    /// Exact fingerprint confirmed by the member.
    pub fingerprint: String,
    /// Original Run whose isolation ref owns the retained worktree.
    pub source_run_id: String,
}

/// One workspace-scoped invocation of a complete coding Agent harness.
#[derive(Clone, Debug)]
pub struct WorkspaceAgentInvocation {
    /// Stable correlation identity for the external turn.
    pub request_id: String,
    /// Provider-specific model selected by the pinned Agent revision.
    pub model: String,
    /// Optional model-supported reasoning effort fixed for this Run.
    pub reasoning_effort: Option<String>,
    /// Immutable Project system instructions, separate from conversation text.
    pub project_instructions: Option<String>,
    /// Conversation path and current user task, excluding system instructions.
    pub prompt: String,
    /// Short subject used when the harness produced a Git commit.
    pub commit_subject: String,
    /// Canonical Project Git root and sandbox boundary.
    pub cwd: PathBuf,
    /// Exact manager-owned failed-Run workspace explicitly adopted by the member.
    pub adopted_worktree: Option<AdoptedWorkspace>,
    /// Full Git HEAD captured while the workspace write lease was held.
    ///
    /// A workspace-writing adapter must run from this immutable baseline and
    /// refuse to integrate its result if the Project worktree moves away from
    /// it during the invocation.
    pub baseline_commit: String,
    /// Exact index tree captured with `baseline_commit` at write admission.
    pub baseline_index_tree: String,
    /// Cooperative cancellation shared with the caller.
    pub cancellation: CancellationToken,
    /// Shared cancellation/finalization decision owned by the application supervisor.
    pub integration_gate: Option<Arc<dyn WorkspaceIntegrationGate>>,
}

/// Durable-facing result of one workspace Agent turn.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkspaceAgentResponse {
    /// Final assistant result shown in the Session.
    pub assistant_text: String,
    /// Commit created for workspace changes, when the turn changed files.
    pub commit_id: Option<String>,
    /// Bounded, display-only records for native harness operations.
    ///
    /// These records preserve user-visible audit context without pretending
    /// that harness-owned tools were executed through Ait's `ToolExecution`
    /// lifecycle.
    pub operations: Vec<WorkspaceOperation>,
    /// Ordered, display-only projection of harness messages and operations.
    ///
    /// Message entries retain provider message boundaries and phases. Operation
    /// entries reference `operations` by their harness-stable identity so the
    /// audit records stay separate from Ait's host tool lifecycle.
    pub output_items: Vec<WorkspaceOutputItem>,
}

/// One provider-normalized, user-visible update from a workspace harness.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WorkspaceProgressEvent {
    /// A provider-authored message item became visible.
    MessageStarted {
        /// Harness-stable item identity.
        id: String,
        /// Provider phase such as `commentary` or `final_answer`.
        phase: Option<String>,
        /// Optional initial full text.
        text: String,
    },
    /// More text arrived for one message item.
    TextDelta {
        /// Harness-stable item identity.
        id: String,
        /// Ordered text suffix.
        delta: String,
    },
    /// The provider supplied the authoritative full message item.
    MessageCompleted {
        /// Harness-stable item identity.
        id: String,
        /// Provider phase such as `commentary` or `final_answer`.
        phase: Option<String>,
        /// Authoritative full text.
        text: String,
    },
    /// A native operation started or changed state.
    OperationStarted(WorkspaceOperation),
    /// A native operation reached a provider terminal state.
    OperationCompleted(WorkspaceOperation),
    /// A safe, user-visible provider warning or retry notice.
    Warning {
        /// Bounded diagnostic text.
        message: String,
        /// Whether the provider reports an automatic retry.
        retrying: bool,
        /// Optional provider-normalized error code.
        code: Option<String>,
    },
    /// The underlying provider turn changed state.
    TurnStatus {
        /// Stable lowercase status.
        status: String,
        /// Safe terminal diagnostic, when present.
        error: Option<String>,
    },
}

/// Progress sink supplied by the application to a provider adapter.
#[async_trait]
pub trait WorkspaceProgressReporter: Send + Sync {
    /// Accepts one ordered update. Implementations may batch persistence, but
    /// must preserve order within a Run.
    async fn report(&self, event: WorkspaceProgressEvent);
}

/// One ordered item in a workspace harness' durable display projection.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WorkspaceOutputItem {
    /// A provider-authored progress or final-answer message.
    Message {
        /// Harness-stable message item identity.
        id: String,
        /// Provider phase such as `commentary` or `final_answer`.
        phase: Option<String>,
        /// Reconciled full text for this one message item.
        text: String,
    },
    /// A native harness operation, referenced by `WorkspaceOperation::id`.
    Operation {
        /// Harness-stable operation item identity.
        id: String,
    },
}

/// Safe projection of one operation performed inside a workspace Agent harness.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkspaceOperation {
    /// Harness-stable item identity when one was supplied.
    pub id: String,
    /// Stable operation category such as `read`, `search`, or `file_change`.
    pub kind: String,
    /// Completion state reported by the harness.
    pub status: String,
    /// Short human-readable action label.
    pub title: String,
    /// Optional bounded target, query, or command summary.
    pub summary: Option<String>,
    /// Optional bounded command output, diff, or result detail.
    pub detail: Option<String>,
    /// Project file references associated with the operation.
    pub paths: Vec<String>,
}

/// Input for a small, read-only Session-title generation turn.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionTitleRequest {
    /// Stable correlation identity for the external turn.
    pub request_id: String,
    /// At most 2,000 user-prompt characters, without transport instructions.
    pub user_prompt: String,
    /// Canonical Project root used only as the harness working directory.
    pub cwd: PathBuf,
    /// Cooperative cancellation shared with the caller.
    pub cancellation: CancellationToken,
}

/// Searchable metadata returned by a Session-title generation turn.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GeneratedSessionTitle {
    /// Short visible Session title.
    pub title: String,
    /// Compact natural-language description used by Session search.
    pub description: String,
}

/// Read-only model boundary for generating Session metadata.
#[async_trait]
pub trait SessionTitleGenerator: Send + Sync {
    /// Generates one title and search description for a Session.
    async fn generate(
        &self,
        request: SessionTitleRequest,
    ) -> Result<GeneratedSessionTitle, DomainError>;
}

/// Complete coding-harness boundary used by the local control-plane slice.
#[async_trait]
pub trait WorkspaceAgent: Send + Sync {
    /// Runs the selected harness and commits any generated workspace changes.
    ///
    /// Returning is also the execution settlement barrier: implementations
    /// must not return until every process owned by the invocation has exited
    /// and any Git side effect that already started has a known outcome.
    /// Cancellation requests cooperative interruption, but does not permit the
    /// caller to drop this future and assume the external work was undone.
    async fn invoke(
        &self,
        request: WorkspaceAgentInvocation,
    ) -> Result<WorkspaceAgentResponse, DomainError>;

    /// Runs the harness while forwarding normalized user-visible progress.
    /// Adapters without streaming support retain their existing behavior.
    async fn invoke_with_progress(
        &self,
        request: WorkspaceAgentInvocation,
        _progress: Arc<dyn WorkspaceProgressReporter>,
    ) -> Result<WorkspaceAgentResponse, DomainError> {
        self.invoke(request).await
    }
}

/// A tool invocation with stable host-assigned idempotency identity.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolInvocation {
    /// Run containing the call.
    pub run_id: RunId,
    /// Provider-stable call identity.
    pub call_id: String,
    /// Host-stable execution attempt identity.
    pub execution_id: ToolExecutionId,
    /// Registered tool name.
    pub tool_name: String,
    /// Canonical arguments.
    pub arguments: Value,
    /// Cooperative cancellation shared with the Run supervisor.
    pub cancellation: CancellationToken,
}

/// Normalized successful tool output.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ToolOutcome {
    /// Bounded structured output suitable for a `ToolResult` Message.
    pub output: Value,
}

/// Reconciliation result for an execution interrupted after dispatch.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ToolRecovery {
    /// The external effect is known to have completed.
    Completed(ToolOutcome),
    /// The idempotency key makes dispatching the same attempt safe.
    RetrySafe,
    /// The external effect cannot be determined safely.
    Unknown,
}

/// Tool catalog/execution boundary consumed by the coordinator.
#[async_trait]
pub trait RunTool: Send + Sync {
    /// Returns whether host policy requires an approval for this call.
    fn requires_approval(&self, tool_name: &str, arguments: &Value) -> bool;

    /// Executes a previously persisted tool intent.
    async fn execute(&self, request: ToolInvocation) -> Result<ToolOutcome, DomainError>;

    /// Reconciles a persisted Running execution after process recovery.
    async fn reconcile(&self, execution: &ToolExecution) -> Result<ToolRecovery, DomainError>;
}

/// Human/policy approval request for a persisted `ToolExecution`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ApprovalRequest {
    /// Owning Run.
    pub run_id: RunId,
    /// Tool execution awaiting a decision.
    pub execution: ToolExecution,
}

/// Result of consulting approval policy or an interactive approver.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApprovalDecision {
    /// Explicit authorization was granted.
    Approved,
    /// Explicit authorization was denied.
    Denied,
    /// No decision exists yet; leave the Run resumably waiting.
    Pending,
}

/// Approval boundary consumed by the coordinator.
#[async_trait]
pub trait RunApproval: Send + Sync {
    /// Resolves or observes the current decision.
    async fn decide(&self, request: ApprovalRequest) -> Result<ApprovalDecision, DomainError>;
}

/// Time boundary used for deadlines and deterministic retry tests.
#[async_trait]
pub trait RunClock: Send + Sync {
    /// Returns current wall-clock time.
    fn now(&self) -> TimestampMs;

    /// Waits until a persisted retry becomes due.
    async fn sleep_until(&self, deadline: TimestampMs);
}

/// Identity boundary used to make every durable child externally assignable.
pub trait RunIdGenerator: Send + Sync {
    /// Creates an immutable Message identity.
    fn message_id(&self) -> MessageId;
    /// Creates an attempt identity.
    fn attempt_id(&self) -> RunAttemptId;
    /// Creates a tool execution identity.
    fn tool_execution_id(&self) -> ToolExecutionId;
}
