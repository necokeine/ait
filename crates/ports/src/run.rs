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
