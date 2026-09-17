use std::{path::PathBuf, pin::Pin, sync::Arc};

use async_trait::async_trait;
use futures_core::Stream;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use ait_domain::NativeApprovalTarget;

use crate::AdapterError;

/// Asynchronous stream of normalized agent events.
pub type AgentStream =
    Pin<Box<dyn Stream<Item = Result<AgentEvent, AdapterError>> + Send + 'static>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
/// Variants represented by `SandboxMode`.
pub enum SandboxMode {
    /// Selects the `ReadOnly` variant.
    ReadOnly,
    /// Selects the `WorkspaceWrite` variant.
    WorkspaceWrite,
    /// Selects the `DangerFullAccess` variant.
    DangerFullAccess,
}

impl SandboxMode {
    pub(crate) fn as_wire_value(self) -> &'static str {
        match self {
            Self::ReadOnly => "read-only",
            Self::WorkspaceWrite => "workspace-write",
            Self::DangerFullAccess => "danger-full-access",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
/// Variants represented by `ApprovalPolicy`.
pub enum ApprovalPolicy {
    /// Selects the `Untrusted` variant.
    Untrusted,
    /// Selects the `OnRequest` variant.
    OnRequest,
    /// Selects the `Never` variant.
    Never,
}

impl ApprovalPolicy {
    pub(crate) fn as_wire_value(self) -> &'static str {
        match self {
            Self::Untrusted => "untrusted",
            Self::OnRequest => "on-request",
            Self::Never => "never",
        }
    }
}

#[derive(Clone)]
/// Data carried by `AgentRunRequest`.
pub struct AgentRunRequest {
    /// Request identifier.
    pub request_id: String,
    /// Model value.
    pub model: Option<String>,
    /// Reasoning effort value.
    pub reasoning_effort: Option<String>,
    /// Project instruction snapshot; never assembled from user message text.
    pub project_instructions: Option<String>,
    /// Prompt value.
    pub prompt: String,
    /// Cwd value.
    pub cwd: PathBuf,
    /// Resume thread identifier.
    pub resume_thread_id: Option<String>,
    /// Keep a new Codex thread in memory without saving its session history.
    /// Only applies when `resume_thread_id` is `None`.
    pub ephemeral: bool,
    /// Sandbox value.
    pub sandbox: SandboxMode,
    /// Approval policy value.
    pub approval_policy: ApprovalPolicy,
    /// Optional Run-scoped approval handler. Production workspace Runs always supply one.
    pub approval_handler: Option<Arc<dyn ApprovalHandler>>,
    /// Output schema value.
    pub output_schema: Option<Value>,
    /// Cancellation value.
    pub cancellation: CancellationToken,
}

impl std::fmt::Debug for AgentRunRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AgentRunRequest")
            .field("request_id", &self.request_id)
            .field("model", &self.model)
            .field("reasoning_effort", &self.reasoning_effort)
            .field("project_instructions", &self.project_instructions)
            .field("prompt", &self.prompt)
            .field("cwd", &self.cwd)
            .field("resume_thread_id", &self.resume_thread_id)
            .field("ephemeral", &self.ephemeral)
            .field("sandbox", &self.sandbox)
            .field("approval_policy", &self.approval_policy)
            .field(
                "approval_handler",
                &self.approval_handler.as_ref().map(|_| "<handler>"),
            )
            .field("output_schema", &self.output_schema)
            .field("cancellation", &self.cancellation)
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[allow(clippy::struct_excessive_bools)]
/// Data carried by `AgentCapabilities`.
pub struct AgentCapabilities {
    /// Streaming value.
    pub streaming: bool,
    /// Thread resume value.
    pub thread_resume: bool,
    /// Approvals value.
    pub approvals: bool,
    /// Command execution value.
    pub command_execution: bool,
    /// File changes value.
    pub file_changes: bool,
    /// Usage value.
    pub usage: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
/// Data carried by `AgentUsage`.
pub struct AgentUsage {
    /// Input tokens value.
    pub input_tokens: u64,
    /// Cached input tokens value.
    pub cached_input_tokens: u64,
    /// Output tokens value.
    pub output_tokens: u64,
    /// Reasoning output tokens value.
    pub reasoning_output_tokens: u64,
    /// Total tokens value.
    pub total_tokens: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
/// Variants represented by `AgentRunStatus`.
pub enum AgentRunStatus {
    /// Selects the `Completed` variant.
    Completed,
    /// Selects the `Interrupted` variant.
    Interrupted,
    /// Selects the `Failed` variant.
    Failed,
    /// Selects the `InProgress` variant.
    InProgress,
    /// Selects the `Unknown` variant.
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
/// Variants represented by `ApprovalKind`.
pub enum ApprovalKind {
    /// Selects the `CommandExecution` variant.
    CommandExecution,
    /// Selects the `FileChange` variant.
    FileChange,
    /// Selects the `Permissions` variant.
    Permissions,
    /// Selects the `LegacyCommand` variant.
    LegacyCommand,
    /// Selects the `LegacyPatch` variant.
    LegacyPatch,
    /// Selects the `Unsupported` variant.
    Unsupported,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
/// Data carried by `ApprovalRequest`.
pub struct ApprovalRequest {
    /// Request identifier.
    pub request_id: Value,
    /// Method value.
    pub method: String,
    /// Kind value.
    pub kind: ApprovalKind,
    /// Thread identifier.
    pub thread_id: String,
    /// Turn identifier.
    pub turn_id: String,
    /// Item identifier.
    pub item_id: String,
    /// Target value.
    pub target: NativeApprovalTarget,
    /// Params value.
    pub params: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
/// Variants represented by `ApprovalDecision`.
pub enum ApprovalDecision {
    /// Selects the `Accept` variant.
    Accept,
    /// Selects the `AcceptForSession` variant.
    AcceptForSession,
    /// Selects the `Decline` variant.
    Decline,
    /// Selects the `Cancel` variant.
    Cancel,
    /// Escape hatch for protocol additions not yet normalized by this crate.
    Raw(Value),
}

#[async_trait]
/// Behavior provided by `ApprovalHandler`.
pub trait ApprovalHandler: Send + Sync {
    /// Returns the host decision for one approval request.
    async fn decide(&self, request: &ApprovalRequest) -> ApprovalDecision;

    /// Notifies the host that Codex withdrew a request before its answer was used.
    async fn resolved(&self, _request: &ApprovalRequest) {}
}

#[derive(Debug, Default)]
/// Data carried by `DenyAllApprovals`.
pub struct DenyAllApprovals;

#[async_trait]
impl ApprovalHandler for DenyAllApprovals {
    async fn decide(&self, _request: &ApprovalRequest) -> ApprovalDecision {
        ApprovalDecision::Decline
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
/// Variants represented by `AgentEvent`.
pub enum AgentEvent {
    /// Selects the `ThreadStarted` variant.
    ThreadStarted {
        /// Thread identifier.
        thread_id: String,
    },
    /// Selects the `TurnStarted` variant.
    TurnStarted {
        /// Turn identifier.
        turn_id: String,
    },
    /// Selects the `MessageDelta` variant.
    MessageDelta {
        /// Item identifier.
        item_id: String,
        /// Delta value.
        delta: String,
    },
    /// Selects the `ItemStarted` variant.
    ItemStarted {
        /// Item value.
        item: Value,
    },
    /// Selects the `ItemCompleted` variant.
    ItemCompleted {
        /// Item value.
        item: Value,
    },
    /// Selects the `ApprovalRequested` variant.
    ApprovalRequested {
        /// Request value.
        request: ApprovalRequest,
    },
    /// Selects the `Usage` variant.
    Usage {
        /// Usage value.
        usage: AgentUsage,
    },
    /// Selects the `AdapterWarning` variant.
    AdapterWarning {
        /// Message value.
        message: String,
        /// Retrying value.
        retrying: bool,
        /// Code value.
        code: Option<String>,
    },
    /// Selects the `Completed` variant.
    Completed {
        /// Turn identifier.
        turn_id: String,
        /// Status value.
        status: AgentRunStatus,
        /// Error value.
        error: Option<String>,
    },
    /// Selects the `RawNotification` variant.
    RawNotification {
        /// Method value.
        method: String,
        /// Params value.
        params: Value,
    },
}

#[async_trait]
/// Behavior provided by `AgentAdapter`.
pub trait AgentAdapter: Send + Sync {
    /// Returns the stable adapter driver identifier.
    fn driver(&self) -> &'static str;
    /// Returns capabilities implemented by the adapter.
    fn capabilities(&self) -> AgentCapabilities;
    /// Starts an agent Run and returns its normalized event stream.
    ///
    /// # Errors
    ///
    /// Returns [`AdapterError`] when the Run cannot be started.
    async fn run(&self, request: AgentRunRequest) -> Result<AgentStream, AdapterError>;
}
