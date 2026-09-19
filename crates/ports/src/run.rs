use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};

use ait_domain::{
    ApprovalGrantScope, DomainError, Message, MessageId, NativeApprovalKind, NativeApprovalTarget,
    ProjectedMessage, Run, RunAttempt, RunAttemptId, RunId, RunPermissionProfile, RunUsage,
    TimestampMs, ToolExecution, ToolExecutionId,
};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
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
    /// Present a member-facing tool interaction for this exact worker lease.
    async fn request_tool_interaction(
        &self,
        _lease: &crate::WorkerLease,
        _call_id: String,
        _execution_id: String,
        _tool_name: String,
        _arguments: Value,
    ) -> Result<ToolOutcome, DomainError> {
        Err(DomainError::invariant(
            ait_domain::ErrorCode::ToolExecutionFailed,
            "tool interaction port unavailable",
        ))
    }

    /// Reconcile a previously persisted member interaction.
    async fn recover_tool_interaction(
        &self,
        _execution: &ToolExecution,
    ) -> Result<ToolRecovery, DomainError> {
        Ok(ToolRecovery::Unknown)
    }

    /// Ask the application to authorize this exact persisted intent under a live lease.
    async fn request_tool_approval(
        &self,
        _lease: &crate::WorkerLease,
        _execution: ToolExecution,
    ) -> Result<ApprovalDecision, DomainError> {
        Ok(ApprovalDecision::Denied)
    }

    /// Consume an approved grant durably before a worker can execute it.
    async fn consume_tool_grant(
        &self,
        _lease: &crate::WorkerLease,
        _grant: &ait_domain::ToolGrant,
    ) -> Result<bool, DomainError> {
        Ok(false)
    }

    /// Release approval waits after a worker connection is lost.
    fn interrupt_tool_approvals(&self, _lease: &crate::WorkerLease) {}
    /// Bound visible approval expiry by the supervisor's actual total deadline.
    fn set_worker_deadline(&self, _deadline: i64) {}

    /// Claims a fresh durable worker lease before spawning an executor.
    async fn claim_worker(&self, _instance: &str) -> Result<crate::WorkerLease, RunStoreError> {
        Err(crate::dispatch::unsupported_worker_store())
    }

    /// Atomically commits a mutation and its immutable idempotency receipt.
    /// Reusing an operation id with different input must fail; old leases fail
    /// even when their operation id has a receipt.
    async fn commit_worker(
        &self,
        _lease: &crate::WorkerLease,
        _operation_id: &str,
        _mutation: crate::RunMutation,
    ) -> Result<crate::RunReceipt, RunStoreError> {
        Err(crate::dispatch::unsupported_worker_store())
    }

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

/// Provider-normalized native approval request associated with one Run and turn.
#[derive(Clone, Debug, PartialEq)]
pub struct WorkspaceApprovalRequest {
    /// Stable Ait Run identifier.
    pub run_id: String,
    /// Original JSON-RPC string or integer id. Other JSON kinds are rejected by the adapter.
    pub protocol_request_id: Value,
    /// Native JSON-RPC method name.
    pub method: String,
    /// Normalized native approval kind.
    pub kind: NativeApprovalKind,
    /// Codex thread identifier.
    pub thread_id: String,
    /// Codex turn identifier.
    pub turn_id: String,
    /// Codex item identifier.
    pub item_id: String,
    /// Bounded, redacted object a member can review before deciding.
    pub target: NativeApprovalTarget,
    /// Exact protocol permission profile, only for permission requests.
    pub requested_permissions: Option<Value>,
}

/// Answer returned to the native harness after durable authorization recording.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WorkspaceApprovalDecision {
    /// The member explicitly approved the request with the recorded scope.
    Approved {
        /// Provider-supported one-shot or session grant scope.
        scope: ApprovalGrantScope,
        /// Exact, validated permission profile for permission requests.
        permissions: Option<Value>,
    },
    /// The member explicitly rejected the request.
    Denied,
    /// The request was cancelled rather than authorized.
    Cancelled,
}

/// Application-owned persistence and user-decision boundary for native approvals.
#[async_trait]
pub trait WorkspaceApproval: Send + Sync {
    /// Persists the request, publishes it to clients, and waits without blocking event dispatch.
    async fn decide(
        &self,
        request: WorkspaceApprovalRequest,
    ) -> Result<WorkspaceApprovalDecision, DomainError>;

    /// Expires a still-pending request withdrawn by the harness or its turn.
    async fn expire(&self, request: &WorkspaceApprovalRequest) -> Result<(), DomainError>;
}

/// Fail-closed approval port for callers that cannot surface native prompts.
#[derive(Clone, Copy, Debug, Default)]
pub struct DenyWorkspaceApprovals;

#[async_trait]
impl WorkspaceApproval for DenyWorkspaceApprovals {
    async fn decide(
        &self,
        _request: WorkspaceApprovalRequest,
    ) -> Result<WorkspaceApprovalDecision, DomainError> {
        Ok(WorkspaceApprovalDecision::Denied)
    }

    async fn expire(&self, _request: &WorkspaceApprovalRequest) -> Result<(), DomainError> {
        Ok(())
    }
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
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
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
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
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
    /// Fixed Small Agent configuration selected by the application.
    pub config: ait_domain::AgentConfiguration,
    /// Provider resolved from the selected Small Agent.
    pub provider: ait_domain::AgentProvider,
    /// Opaque credential reference for API providers; never a secret.
    pub credential_ref: Option<String>,
    /// Canonical Project root used only as the harness working directory.
    pub cwd: PathBuf,
    /// Cooperative cancellation shared with the caller.
    pub cancellation: CancellationToken,
}

/// Searchable metadata returned by a Session-title generation turn.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
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

/// Accumulates nested provider/tool usage even when a tool later fails,
/// is cancelled, or exceeds a limit.
#[derive(Clone, Default)]
pub struct ToolUsageRecorder(Arc<Mutex<RunUsage>>);

impl ToolUsageRecorder {
    /// Adds known nested usage using saturating arithmetic.
    pub fn record(&self, delta: &RunUsage) {
        let mut total = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        total.input_tokens = total.input_tokens.saturating_add(delta.input_tokens);
        total.cached_input_tokens = total
            .cached_input_tokens
            .saturating_add(delta.cached_input_tokens);
        total.output_tokens = total.output_tokens.saturating_add(delta.output_tokens);
        total.tool_executions = total.tool_executions.saturating_add(delta.tool_executions);
        total.cost = match (total.cost, delta.cost) {
            (None, None) => None,
            (left, right) => Some(ait_domain::CostMicros(
                left.map_or(0, ait_domain::CostMicros::get)
                    .saturating_add(right.map_or(0, ait_domain::CostMicros::get)),
            )),
        };
    }

    /// Returns all usage recorded so far.
    #[must_use]
    pub fn snapshot(&self) -> RunUsage {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

impl std::fmt::Debug for ToolUsageRecorder {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_tuple("ToolUsageRecorder")
            .field(&self.snapshot())
            .finish()
    }
}

impl PartialEq for ToolUsageRecorder {
    fn eq(&self, other: &Self) -> bool {
        self.snapshot() == other.snapshot()
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
    /// Nested usage sink observed by the coordinator on every exit path.
    pub usage: ToolUsageRecorder,
    /// Cooperative cancellation shared with the Run supervisor.
    pub cancellation: CancellationToken,
}

/// Normalized successful tool output.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ToolOutcome {
    /// Bounded structured output suitable for a `ToolResult` Message.
    pub output: Value,
    /// Provider usage incurred inside this tool, such as a foreground child Agent.
    pub usage: RunUsage,
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
    /// Names the host can actually execute. Empty by default for older adapters.
    fn executable_tools(&self) -> Vec<String> {
        Vec::new()
    }

    /// Whether calls can overlap without changing their observable effects.
    fn parallel_safe(&self, _tool_name: &str, _arguments: &Value) -> bool {
        false
    }

    /// Returns whether host policy requires an approval for this call.
    fn requires_approval(&self, tool_name: &str, arguments: &Value) -> bool;

    /// Executes a previously persisted tool intent.
    async fn execute(&self, request: ToolInvocation) -> Result<ToolOutcome, DomainError>;

    /// Execute one consumed grant after rechecking its complete binding and real targets.
    async fn execute_granted(
        &self,
        _request: ToolInvocation,
        _grant: ait_domain::ToolGrant,
    ) -> Result<ToolOutcome, DomainError> {
        Err(DomainError::invariant(
            ait_domain::ErrorCode::ToolApprovalRequired,
            "executor does not support one-operation grants",
        ))
    }

    /// Cancels and joins all owned work before a Run may become terminal.
    /// Adapters that start work surviving a dropped execute future must override
    /// this method. Return only once no owned worker can produce a late effect.
    async fn cancel_and_drain(&self) {}

    /// Reconciles a persisted Running execution after process recovery.
    async fn reconcile(&self, execution: &ToolExecution) -> Result<ToolRecovery, DomainError>;
}

/// Application-owned interaction boundary used by tools that must wait for a
/// member decision. The request identity is the durable `ToolExecution` id, so
/// reconnects and worker retries observe one answer rather than creating a
/// second prompt.
#[async_trait]
pub trait RunToolInteraction: Send + Sync {
    /// Present a bounded question or plan review request and wait for its answer.
    async fn request(&self, request: ToolInvocation) -> Result<ToolOutcome, DomainError>;

    /// Reconcile an interrupted request without replaying a member-visible prompt.
    async fn reconcile(&self, execution: &ToolExecution) -> Result<ToolRecovery, DomainError>;

    /// Release outstanding waits owned by this executor.
    async fn cancel_and_drain(&self) {}
}

/// Combines independently implemented tool families behind one executor.
pub struct CompositeRunTool {
    primary: Arc<dyn RunTool>,
    extension: Arc<dyn RunTool>,
}

impl CompositeRunTool {
    /// Creates a deterministic union; the extension owns any duplicate name.
    #[must_use]
    pub fn new(primary: Arc<dyn RunTool>, extension: Arc<dyn RunTool>) -> Self {
        Self { primary, extension }
    }

    fn extension_owns(&self, name: &str) -> bool {
        self.extension
            .executable_tools()
            .iter()
            .any(|candidate| candidate == name)
    }
}

#[async_trait]
impl RunTool for CompositeRunTool {
    fn executable_tools(&self) -> Vec<String> {
        let mut names = self.primary.executable_tools();
        for name in self.extension.executable_tools() {
            if !names.contains(&name) {
                names.push(name);
            }
        }
        names.sort();
        names
    }

    fn parallel_safe(&self, name: &str, arguments: &Value) -> bool {
        if self.extension_owns(name) {
            self.extension.parallel_safe(name, arguments)
        } else {
            self.primary.parallel_safe(name, arguments)
        }
    }

    fn requires_approval(&self, name: &str, arguments: &Value) -> bool {
        if self.extension_owns(name) {
            self.extension.requires_approval(name, arguments)
        } else {
            self.primary.requires_approval(name, arguments)
        }
    }

    async fn execute(&self, request: ToolInvocation) -> Result<ToolOutcome, DomainError> {
        if self.extension_owns(&request.tool_name) {
            self.extension.execute(request).await
        } else {
            self.primary.execute(request).await
        }
    }

    async fn execute_granted(
        &self,
        request: ToolInvocation,
        grant: ait_domain::ToolGrant,
    ) -> Result<ToolOutcome, DomainError> {
        if self.extension_owns(&request.tool_name) {
            self.extension.execute_granted(request, grant).await
        } else {
            self.primary.execute_granted(request, grant).await
        }
    }

    async fn cancel_and_drain(&self) {
        self.primary.cancel_and_drain().await;
        self.extension.cancel_and_drain().await;
    }

    async fn reconcile(&self, execution: &ToolExecution) -> Result<ToolRecovery, DomainError> {
        if self.extension_owns(&execution.tool_name) {
            self.extension.reconcile(execution).await
        } else {
            self.primary.reconcile(execution).await
        }
    }
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
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ApprovalDecision {
    /// Explicit authorization was granted.
    Approved,
    /// An explicit, auditable one-operation grant. It still requires consumption.
    Granted(Box<ait_domain::ToolGrant>),
    /// The entire Run was cancelled while waiting.
    Cancelled,
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

    /// Revalidate and consume one grant immediately before execution.
    async fn consume(&self, _grant: &ait_domain::ToolGrant) -> Result<bool, DomainError> {
        Ok(false)
    }
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

/// Factory for host executors pinned to the admitted Project and permission ceiling.
pub trait RunToolFactory: Send + Sync {
    /// Build a bounded authorization target using the same backend as execution.
    /// Returns no target for unsupported, unreviewable, or non-escalating operations.
    /// # Errors
    /// Returns a safe denial when validation or filesystem identity checks fail.
    fn review(
        &self,
        _root: &std::path::Path,
        _profile: RunPermissionProfile,
        _execution: &ToolExecution,
    ) -> Result<Option<ait_domain::ToolApprovalTarget>, DomainError> {
        Ok(None)
    }
    /// Assemble a capability-scoped executor; no side effects occur at creation.
    ///
    /// # Errors
    /// Returns a safe error if the Project capability cannot be opened.
    fn create(
        &self,
        root: &std::path::Path,
        profile: RunPermissionProfile,
    ) -> Result<Arc<dyn RunTool>, DomainError>;

    /// Layer Agent-backed delegation and member interactions over host tools.
    /// Factories without those capabilities keep the primary executor unchanged.
    fn extend_agent_tools(
        &self,
        primary: Arc<dyn RunTool>,
        _child_agent: Arc<dyn RunAgent>,
        _interactions: Arc<dyn RunToolInteraction>,
    ) -> Arc<dyn RunTool> {
        primary
    }
}
