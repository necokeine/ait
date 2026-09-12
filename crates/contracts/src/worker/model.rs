//! Frozen worker v1 value records. No domain or SDK types.
#![allow(clippy::large_enum_variant)]
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

/// Worker v1 file change kind.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NativeApprovalFileChangeKind {
    /// New path.
    Add,
    /// Removed path.
    Delete,
    /// Updated path.
    Update,
}

/// Worker v1 Run record; independent of domain serialization.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Run {
    /// Run identity.
    pub id: String,
    /// Owning Project.
    pub project_id: String,
    /// Immutable starting Message.
    pub base_message_id: String,
    /// Last Message persisted by this Run, when any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_message_id: Option<String>,
    /// Session advanced by outputs, absent for Cron/background Runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub follow_session_id: Option<String>,
    /// Fixed Agent identity.
    pub agent_id: String,
    /// Fixed Agent revision.
    pub agent_revision: u64,
    /// Reproducible non-secret copy of that exact revision.
    pub agent_snapshot: AgentConfigSnapshot,
    /// Trigger class.
    pub trigger: RunTrigger,
    /// Source Cron for a scheduled Run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cron_id: Option<String>,
    /// Scheduled occurrence for a Cron Run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scheduled_at: Option<i64>,
    /// Coarse lifecycle state.
    pub status: RunStatus,
    /// Current lifecycle phase.
    pub phase: RunPhase,
    /// Terminal reason, present only in a terminal state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<RunStopReason>,
    /// Safe terminal or recoverable failure information.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<DomainError>,
    /// Persisted steps completed so far.
    pub step_count: u64,
    /// Fixed limits.
    pub budget: RunBudget,
    /// Persisted cumulative usage.
    #[serde(default)]
    pub usage: RunUsage,
    /// Attempts started so far.
    pub attempt_count: u32,
    /// Context compactions completed so far.
    pub compaction_count: u32,
    /// Fixed retry policy.
    pub retry_policy: RetryPolicy,
    /// Due time while waiting to retry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_retry_at: Option<i64>,
    /// Latest durable recovery checkpoint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checkpoint_id: Option<String>,
    /// Version incremented whenever work is enqueued.
    pub queue_version: u64,
    /// Last consumed queue sequence.
    pub queue_cursor: u64,
    /// Optional idempotency key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dedupe_key: Option<String>,
    /// First execution time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<i64>,
    /// Terminal time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<i64>,
    /// Creation time.
    pub created_at: i64,
}

/// Worker v1 `RunAttempt` record; independent of domain serialization.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunAttempt {
    /// Attempt identity.
    pub id: String,
    /// Owning Run.
    pub run_id: String,
    /// Monotonic Run-local number beginning at one.
    pub number: u32,
    /// Why this attempt started.
    pub reason: RunAttemptReason,
    /// Recovery checkpoint used by this attempt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checkpoint_id: Option<String>,
    /// Attempt lifecycle state.
    pub status: RunAttemptStatus,
    /// Safe failure information.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<DomainError>,
    /// Start time.
    pub started_at: i64,
    /// End time for a terminal attempt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<i64>,
}

/// Worker v1 `RunBudget` record; independent of domain serialization.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunBudget {
    /// Maximum persisted Agent/tool steps; must be positive.
    pub max_steps: u64,
    /// Optional total token allowance.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_budget: Option<u64>,
    /// Optional total cost allowance.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_budget: Option<u64>,
    /// Optional wall-clock allowance.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_runtime: Option<u64>,
}

/// Worker v1 `RunUsage` record; independent of domain serialization.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct RunUsage {
    /// Uncached input tokens.
    pub input_tokens: u64,
    /// Cached input tokens.
    pub cached_input_tokens: u64,
    /// Output tokens.
    pub output_tokens: u64,
    /// Tool executions started.
    pub tool_executions: u64,
    /// Billed cost when supplied by an adapter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost: Option<u64>,
}

/// Worker v1 `RetryPolicy` record; independent of domain serialization.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RetryPolicy {
    /// Maximum number of attempts including the initial attempt.
    pub max_attempts: u32,
    /// Initial delay before retrying.
    pub initial_delay: u64,
    /// Maximum delay after backoff.
    pub max_delay: u64,
}

/// Worker v1 `RunTrigger` record; independent of domain serialization.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Copy, Ord, PartialOrd)]
#[serde(rename_all = "snake_case")]
pub enum RunTrigger {
    /// Explicit interactive or background request.
    Manual,
    /// One scheduled Cron occurrence.
    Cron,
}

/// Worker v1 `RunStatus` record; independent of domain serialization.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Copy, Ord, PartialOrd)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    /// Accepted but not yet executing.
    Queued,
    /// Actively progressing through an execution phase.
    Running,
    /// Waiting for a tool approval decision.
    WaitingApproval,
    /// Waiting until a retry becomes due.
    RetryWait,
    /// Evaluating the atomic termination barrier.
    Settling,
    /// Termination barrier passed successfully.
    Completed,
    /// An unrecoverable error or exhausted retry policy ended the Run.
    Failed,
    /// Cancellation ended the Run.
    Cancelled,
    /// A step, token, cost, or runtime limit ended the Run.
    LimitExceeded,
}

/// Worker v1 `RunPhase` record; independent of domain serialization.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Copy, Ord, PartialOrd)]
#[serde(rename_all = "snake_case")]
pub enum RunPhase {
    /// Waiting for a worker.
    Queued,
    /// Validating and claiming the optional Session reference.
    AcquiringSessionRef,
    /// Building provider input from the Message path.
    AssemblingContext,
    /// Waiting for an Agent invocation.
    CallingAgent,
    /// Atomically persisting an output and advancing the Session.
    PersistingMessageAndAdvancingSession,
    /// Waiting for tool approval.
    WaitingApproval,
    /// Executing a tool.
    ExecutingTool,
    /// Persisting the user `ToolResult` Message.
    PersistingToolResult,
    /// Waiting for a retry time.
    RetryWait,
    /// Compacting context without changing Run identity.
    CompactingContext,
    /// Persisting a recovery checkpoint.
    Checkpointing,
    /// Restoring from a checkpoint.
    Recovering,
    /// Consuming queued work.
    DrainingQueue,
    /// Evaluating the completion barrier.
    Settling,
    /// Conditionally releasing a followed Session.
    ReleasingSessionRef,
    /// Terminal state has been persisted.
    Terminal,
}

/// Worker v1 `RunStopReason` record; independent of domain serialization.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Copy, Ord, PartialOrd)]
#[serde(rename_all = "snake_case")]
pub enum RunStopReason {
    /// All work drained and the termination barrier passed.
    Completed,
    /// User or host cancellation.
    Cancelled,
    /// Maximum Agent steps reached.
    StepLimit,
    /// Token budget reached.
    TokenBudget,
    /// Cost budget reached.
    CostBudget,
    /// Maximum wall-clock runtime reached.
    RuntimeLimit,
    /// Unrecoverable Agent, tool, adapter, or persistence failure.
    Failed,
    /// Retry policy was exhausted.
    RetryExhausted,
}

/// Worker v1 `RunAttemptReason` record; independent of domain serialization.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Copy, Ord, PartialOrd)]
#[serde(rename_all = "snake_case")]
pub enum RunAttemptReason {
    /// First invocation.
    Initial,
    /// Retry after a recoverable failure.
    Retry,
    /// Process or context recovery.
    Recovery,
}

/// Worker v1 `RunAttemptStatus` record; independent of domain serialization.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Copy, Ord, PartialOrd)]
#[serde(rename_all = "snake_case")]
pub enum RunAttemptStatus {
    /// Invocation is active.
    Running,
    /// Invocation returned successfully.
    Completed,
    /// Invocation failed.
    Failed,
    /// Invocation was cancelled.
    Cancelled,
}

/// Worker v1 Message record; independent of domain serialization.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Message {
    /// Message identity.
    pub id: String,
    /// Owning Project.
    pub project_id: String,
    /// Parent Message, absent only for a root System Message.
    pub parent_message_id: Option<String>,
    /// Provider-facing role.
    pub role: MessageRole,
    /// Ordinary or `ToolResult` protocol kind.
    pub kind: MessageKind,
    /// Creation source.
    pub origin: MessageOrigin,
    /// Ordered content parts.
    pub sub_messages: Vec<SubMessage>,
    /// Session that caused creation, for audit only.
    pub created_by_session_id: Option<String>,
    /// Run provenance, when generated during a Run.
    pub run_id: Option<String>,
    /// Monotonic sequence inside `run_id`.
    pub run_seq: Option<u64>,
    /// Fields present only for [`MessageKind::ToolResult`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_result: Option<ToolResult>,
    /// Clean repository HEAD captured for an interactive human user Message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_commit: Option<String>,
    /// Non-secret extension metadata fixed at creation.
    #[serde(default)]
    pub metadata: BTreeMap<String, Value>,
    /// Creation time.
    pub created_at: i64,
}

/// Worker v1 `MessageRole` record; independent of domain serialization.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Copy, Ord, PartialOrd)]
#[serde(rename_all = "snake_case")]
pub enum MessageRole {
    /// Human, scheduler, system-injected, or `ToolResult` input.
    User,
    /// Root instruction snapshot.
    System,
    /// Agent output.
    Assistant,
}

/// Worker v1 `MessageKind` record; independent of domain serialization.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Copy, Ord, PartialOrd)]
#[serde(rename_all = "snake_case")]
pub enum MessageKind {
    /// Ordinary user, system, or assistant content.
    Standard,
    /// A special user Message answering a prior `ToolUse`.
    ToolResult,
}

/// Worker v1 `MessageOrigin` record; independent of domain serialization.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Copy, Ord, PartialOrd)]
#[serde(rename_all = "snake_case")]
pub enum MessageOrigin {
    /// Project instruction discovery.
    Project,
    /// Interactive human input.
    Human,
    /// Agent output.
    Agent,
    /// Tool execution output.
    Tool,
    /// Scheduled input.
    Scheduler,
    /// Host-generated input.
    System,
}

/// Worker v1 `ToolResult` record; independent of domain serialization.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolResult {
    /// Provider-stable `ToolUse` call identity.
    pub call_id: String,
    /// Final execution status.
    pub status: ToolResultStatus,
    /// Bounded structured result, when available.
    pub output: Option<String>,
    /// Bounded error summary, when available.
    pub error: Option<String>,
}

/// Worker v1 `ToolResultStatus` record; independent of domain serialization.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Copy, Ord, PartialOrd)]
#[serde(rename_all = "snake_case")]
pub enum ToolResultStatus {
    /// Tool execution succeeded.
    Succeeded,
    /// Tool execution failed.
    Failed,
    /// Approval was denied.
    Denied,
    /// Tool execution was cancelled.
    Cancelled,
}

/// Worker v1 `ToolUse` record; independent of domain serialization.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolUse {
    /// Provider-stable call identity, unique within its Run.
    pub call_id: String,
    /// Registered tool name.
    pub tool_name: String,
    /// Canonical structured arguments.
    pub arguments: String,
    /// Optional provider-specific, non-secret metadata.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_metadata: Option<String>,
}

/// Worker v1 `SubMessage` record; independent of domain serialization.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SubMessage {
    /// Plain text.
    Text {
        /// Text content.
        text: String,
    },
    /// Reference to an attachment stored outside the Message body.
    FileRef {
        /// Attachment identity.
        attachment_id: String,
        /// MIME media type.
        media_type: String,
        /// Optional display name.
        name: Option<String>,
    },
    /// Tool request emitted inside an assistant Message.
    ToolUse(ToolUse),
    /// Typed structured content encoded in a canonical representation.
    StructuredData {
        /// Content media type.
        media_type: String,
        /// Canonical encoded value.
        value: String,
    },
}

/// Worker v1 `ProjectedMessage` record; independent of domain serialization.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectedMessage {
    /// Full immutable content is visible.
    Visible(Message),
    /// Content is hidden while identity and graph position remain visible.
    Redacted {
        /// Message identity retained for graph continuity.
        id: String,
        /// Owning Project.
        project_id: String,
        /// Parent edge retained for graph continuity.
        parent_message_id: Option<String>,
        /// Role retained so protocol ordering remains interpretable.
        role: MessageRole,
    },
}

/// Worker v1 `ToolExecution` record; independent of domain serialization.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolExecution {
    /// Execution attempt identity.
    pub id: String,
    /// Owning Run.
    pub run_id: String,
    /// Provider-stable `ToolUse` call identity, unique within the Run.
    pub call_id: String,
    /// Assistant Message containing the `ToolUse`.
    pub assistant_message_id: String,
    /// Zero-based index in the Message's ordered sub-messages.
    pub tool_use_index: u32,
    /// Final user `ToolResult` Message, once persisted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_result_message_id: Option<String>,
    /// Registered tool name copied from the `ToolUse`.
    pub tool_name: String,
    /// Canonical structured arguments copied from the `ToolUse`.
    pub arguments: Value,
    /// Attempt number for this `call_id`, beginning at one.
    pub attempt: u32,
    /// Approval lifecycle.
    pub approval_status: ToolApprovalStatus,
    /// Execution lifecycle.
    pub status: ToolExecutionStatus,
    /// Bounded structured result summary.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    /// Safe failure information.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<DomainError>,
    /// Execution start time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<i64>,
    /// Terminal time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<i64>,
    /// Record creation time.
    pub created_at: i64,
}

/// Worker v1 `ToolApprovalStatus` record; independent of domain serialization.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Copy, Ord, PartialOrd)]
#[serde(rename_all = "snake_case")]
pub enum ToolApprovalStatus {
    /// Policy permits execution without approval.
    NotRequired,
    /// Waiting for an explicit decision.
    Pending,
    /// Explicitly approved.
    Approved,
    /// Explicitly denied.
    Denied,
}

/// Worker v1 `ToolExecutionStatus` record; independent of domain serialization.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Copy, Ord, PartialOrd)]
#[serde(rename_all = "snake_case")]
pub enum ToolExecutionStatus {
    /// Created but not running.
    Pending,
    /// Tool process or adapter is active.
    Running,
    /// Tool completed successfully.
    Succeeded,
    /// Tool failed.
    Failed,
    /// Approval was denied.
    Denied,
    /// Execution was cancelled.
    Cancelled,
}

/// Worker v1 `AgentConfigSnapshot` record; independent of domain serialization.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentConfigSnapshot {
    /// Selected Agent.
    pub agent_id: String,
    /// Selected immutable revision.
    pub revision: u64,
    /// Adapter selection key.
    pub driver_type: String,
    /// Host-side non-secret connection name.
    pub connection_name: String,
    /// Provider model identifier.
    pub model: String,
    /// Optional non-secret endpoint override.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
    /// Fixed capability declaration.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub capabilities: BTreeSet<AgentCapability>,
    /// Fixed default parameters.
    #[serde(default)]
    pub default_parameters: BTreeMap<String, Value>,
    /// Fixed tool policy.
    #[serde(default)]
    pub tool_policy: ToolPolicy,
    /// Configuration digest used to verify the snapshot.
    pub config_digest: String,
}

/// Worker v1 `AgentCapability` record; independent of domain serialization.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Copy, Ord, PartialOrd)]
#[serde(rename_all = "snake_case")]
pub enum AgentCapability {
    /// Produces text content.
    Text,
    /// Accepts referenced files.
    FileInput,
    /// Emits structured data.
    StructuredOutput,
    /// Emits tool calls.
    ToolUse,
    /// May emit more than one tool call in a Message.
    ParallelToolUse,
    /// Supports continuation from a persisted checkpoint.
    CheckpointRecovery,
}

/// Worker v1 `ToolPolicy` record; independent of domain serialization.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolPolicy {
    /// Permission used when no exact tool-name override exists.
    pub default: ToolPermission,
    /// Exact registered tool-name overrides in deterministic key order.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub tools: BTreeMap<String, ToolPermission>,
}

/// Worker v1 `ToolPermission` record; independent of domain serialization.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Copy, Ord, PartialOrd)]
#[serde(rename_all = "snake_case")]
pub enum ToolPermission {
    /// Execute without an approval gate.
    Allow,
    /// Pause until explicit approval is granted or denied.
    RequireApproval,
    /// Do not execute the tool.
    Deny,
}

/// Worker v1 `DomainError` record; independent of domain serialization.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DomainError {
    /// Stable machine-readable code.
    pub code: ErrorCode,
    /// Safe, human-readable explanation.
    pub message: String,
    /// Whether retrying the same idempotent operation may succeed.
    pub retryable: bool,
    /// Optional non-secret structured context.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<BTreeMap<String, Value>>,
    /// Optional opaque identifier used to correlate a lower-level cause.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cause_id: Option<String>,
}

/// Worker v1 `ErrorCode` record; independent of domain serialization.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Copy, Ord, PartialOrd)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ErrorCode {
    /// Project path does not exist.
    ProjectPathNotFound,
    /// Project path is not a directory.
    ProjectPathNotDirectory,
    /// Canonical path is already registered.
    ProjectPathAlreadyRegistered,
    /// A requested new Project directory already exists.
    ProjectPathAlreadyExists,
    /// The host's default parent directory cannot be resolved or accessed.
    ProjectDefaultDirectoryUnavailable,
    /// A new Project directory could not be created.
    ProjectDirectoryCreationFailed,
    /// Git initialization failed.
    ProjectGitInitFailed,
    /// Project Git worktree or index contains changes.
    ProjectGitDirty,
    /// Project repository has no readable HEAD commit.
    ProjectGitHeadUnavailable,
    /// Another process owns the Project workspace write lease.
    ProjectWorkspaceBusy,
    /// A file operation escaped the Project boundary.
    ProjectPathOutOfScope,
    /// Project aggregate fields are inconsistent.
    InvalidProject,
    /// Session does not exist.
    SessionNotFound,
    /// Session already follows a non-terminal Run.
    SessionBusy,
    /// Session compare-and-swap failed.
    SessionPointerConflict,
    /// Session and Message belong to different Projects.
    SessionMessageProjectMismatch,
    /// Session aggregate fields are inconsistent.
    InvalidSession,
    /// Message does not exist.
    MessageNotFound,
    /// Message and parent belong to different Projects.
    MessageProjectMismatch,
    /// Root Message is invalid.
    InvalidRootMessage,
    /// Message role is invalid for the operation.
    InvalidMessageRole,
    /// Message UUID is nil or otherwise unusable as an identity.
    InvalidMessageId,
    /// A sub-message is invalid for its containing Message.
    InvalidSubmessageKind,
    /// Run identity and sequence were not supplied together on a Message.
    InvalidMessageRunProvenance,
    /// Immutable Message mutation was attempted.
    MessageImmutable,
    /// `ToolUse` appeared outside an assistant Message.
    ToolUseRequiresAssistant,
    /// `ToolResult` appeared outside a user Message.
    ToolResultRequiresUser,
    /// `ToolResult` envelope is inconsistent.
    ToolResultMessageInvalid,
    /// An interactive human Message omitted its Git HEAD snapshot.
    HumanMessageGitCommitRequired,
    /// Git HEAD provenance appeared on a non-human Message.
    MessageGitCommitNotAllowed,
    /// A Message carried a malformed Git object identity.
    InvalidMessageGitCommit,
    /// Agent does not exist.
    AgentNotFound,
    /// Agent is disabled.
    AgentDisabled,
    /// Agent revision does not exist.
    AgentRevisionNotFound,
    /// Agent lacks a required capability.
    AgentCapabilityUnsupported,
    /// Agent or revision configuration is invalid.
    InvalidAgentConfiguration,
    /// Core configuration or settings are invalid.
    InvalidConfiguration,
    /// Provider invocation failed after adapter normalization.
    ProviderFailed,
    /// Run aggregate fields are inconsistent.
    InvalidRun,
    /// Run cannot be resumed from its current state.
    RunNotResumable,
    /// Run is terminal.
    RunAlreadyTerminal,
    /// Retry allowance is exhausted.
    RunRetryExhausted,
    /// Run recovery failed.
    RunRecoveryFailed,
    /// Run queue compare-and-swap failed.
    RunQueueConflict,
    /// Run was cancelled.
    RunCancelled,
    /// Run exceeded a configured limit.
    RunLimitExceeded,
    /// `ToolUse` cannot be found on the Run path.
    ToolUseNotFound,
    /// Tool call identity is duplicated in a Run.
    ToolCallDuplicate,
    /// `ToolResult` already exists for the call.
    ToolResultDuplicate,
    /// Tool execution and Message belong to different Runs.
    ToolRunMismatch,
    /// Tool execution requires approval.
    ToolApprovalRequired,
    /// Tool execution failed.
    ToolExecutionFailed,
    /// Tool execution aggregate fields are inconsistent.
    InvalidToolExecution,
    /// Cron's Project cannot be used.
    CronProjectUnavailable,
    /// Cron's fixed base Message cannot be used.
    CronBaseMessageUnavailable,
    /// Cron's fixed Agent cannot be used.
    CronAgentUnavailable,
    /// The same Cron occurrence was already claimed.
    CronDuplicateFire,
    /// Cron concurrency policy blocked the occurrence.
    CronConcurrencyBlocked,
    /// Cron configuration is invalid.
    InvalidCron,
}

/// Worker v1 `RunPermissionProfile` record; independent of domain serialization.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunPermissionProfile {
    /// Maximum filesystem access granted to the harness for the complete Run.
    pub sandbox: SandboxAccess,
    /// Native approval policy supplied to the harness for the complete Run.
    pub approval: ApprovalMode,
}

/// Worker v1 `SandboxAccess` record; independent of domain serialization.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Copy, Ord, PartialOrd)]
#[serde(rename_all = "snake_case")]
pub enum SandboxAccess {
    /// The Agent may inspect the Project but cannot modify files.
    ReadOnly,
    /// The Agent may write only inside the isolated Project workspace.
    WorkspaceWrite,
    /// The Agent may access the host without a filesystem sandbox.
    FullAccess,
}

/// Worker v1 `ApprovalMode` record; independent of domain serialization.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Copy, Ord, PartialOrd)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalMode {
    /// Let the harness request approval when it determines escalation is needed.
    OnRequest,
    /// Require approval for commands the harness classifies as untrusted.
    UntrustedOnly,
}

impl Default for ToolPolicy {
    fn default() -> Self {
        Self {
            default: ToolPermission::Deny,
            tools: BTreeMap::new(),
        }
    }
}

/// Worker v1 `ApprovalGrantScope` value.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalGrantScope {
    /// Authorize only the protocol request being answered.
    OneShot,
    /// Authorize the explicit permission set for the current Codex turn.
    Turn,
    /// Authorize matching requests in the current app-server session.
    Session,
}

/// Worker v1 `NativeApprovalKind` value.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NativeApprovalKind {
    /// A command execution request.
    CommandExecution,
    /// A file-change request.
    FileChange,
    /// A request for an explicit filesystem/network permission profile.
    Permissions,
    /// A legacy command approval request.
    LegacyCommand,
    /// A legacy patch approval request.
    LegacyPatch,
}

/// Worker v1 `NativeApprovalTarget` value.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum NativeApprovalTarget {
    /// One concrete shell command and its working directory.
    Command {
        /// Redacted command text or argument projection.
        command: String,
        /// Project-relative execution context expressed as an absolute host path.
        cwd: String,
    },
    /// One managed-network destination. Network prompts are never rendered as commands.
    Network {
        /// Destination host requested by the managed network proxy.
        host: String,
        /// Network protocol requested for the destination.
        protocol: NativeNetworkProtocol,
    },
    /// Proposed file targets, without persisting patch contents.
    FileChange {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        /// Optional directory scope that Codex asks to retain for later writes.
        grant_root: Option<String>,
        /// Proposed paths and operation kinds, excluding patch contents.
        changes: Vec<NativeApprovalFileChange>,
    },
    /// The working directory associated with an explicit permission-profile request.
    Permissions {
        /// Project working directory to which the permission profile applies.
        cwd: String,
    },
}

/// Worker v1 `NativeNetworkProtocol` value.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NativeNetworkProtocol {
    /// Plain HTTP.
    Http,
    /// HTTP over TLS.
    Https,
    /// SOCKS5 TCP transport.
    Socks5Tcp,
    /// SOCKS5 UDP transport.
    Socks5Udp,
}

/// Worker v1 `NativeApprovalFileChange` value.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeApprovalFileChange {
    /// Proposed file path.
    pub path: String,
    /// Proposed operation on the path.
    pub kind: NativeApprovalFileChangeKind,
}

/// Worker v1 `WorkspaceAgentResponse` value.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
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

/// Worker v1 `WorkspaceOperation` value.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
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

/// Worker v1 `WorkspaceOutputItem` value.
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

/// Worker v1 `WorkspaceProgressEvent` value.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
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

/// Worker v1 `WorkspaceApprovalRequest` value.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
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

/// Worker v1 `WorkspaceApprovalDecision` value.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
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
