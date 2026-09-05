use std::{path::PathBuf, pin::Pin};

use async_trait::async_trait;
use futures_core::Stream;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::AdapterError;

pub type AgentStream =
    Pin<Box<dyn Stream<Item = Result<AgentEvent, AdapterError>> + Send + 'static>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SandboxMode {
    ReadOnly,
    WorkspaceWrite,
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
pub enum ApprovalPolicy {
    Untrusted,
    OnRequest,
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

#[derive(Debug, Clone)]
pub struct AgentRunRequest {
    pub request_id: String,
    pub model: Option<String>,
    pub reasoning_effort: Option<String>,
    pub prompt: String,
    pub cwd: PathBuf,
    pub resume_thread_id: Option<String>,
    pub sandbox: SandboxMode,
    pub approval_policy: ApprovalPolicy,
    pub output_schema: Option<Value>,
    pub cancellation: CancellationToken,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[allow(clippy::struct_excessive_bools)]
pub struct AgentCapabilities {
    pub streaming: bool,
    pub thread_resume: bool,
    pub approvals: bool,
    pub command_execution: bool,
    pub file_changes: bool,
    pub usage: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentUsage {
    pub input_tokens: u64,
    pub cached_input_tokens: u64,
    pub output_tokens: u64,
    pub reasoning_output_tokens: u64,
    pub total_tokens: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AgentRunStatus {
    Completed,
    Interrupted,
    Failed,
    InProgress,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalKind {
    CommandExecution,
    FileChange,
    Permissions,
    LegacyCommand,
    LegacyPatch,
    Unsupported,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApprovalRequest {
    pub request_id: Value,
    pub method: String,
    pub kind: ApprovalKind,
    pub params: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalDecision {
    Accept,
    AcceptForSession,
    Decline,
    Cancel,
    /// Escape hatch for protocol additions not yet normalized by this crate.
    Raw(Value),
}

#[async_trait]
pub trait ApprovalHandler: Send + Sync {
    async fn decide(&self, request: &ApprovalRequest) -> ApprovalDecision;
}

#[derive(Debug, Default)]
pub struct DenyAllApprovals;

#[async_trait]
impl ApprovalHandler for DenyAllApprovals {
    async fn decide(&self, _request: &ApprovalRequest) -> ApprovalDecision {
        ApprovalDecision::Decline
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentEvent {
    ThreadStarted {
        thread_id: String,
    },
    TurnStarted {
        turn_id: String,
    },
    MessageDelta {
        item_id: String,
        delta: String,
    },
    ItemStarted {
        item: Value,
    },
    ItemCompleted {
        item: Value,
    },
    ApprovalRequested {
        request: ApprovalRequest,
    },
    Usage {
        usage: AgentUsage,
    },
    AdapterWarning {
        message: String,
        retrying: bool,
        code: Option<String>,
    },
    Completed {
        turn_id: String,
        status: AgentRunStatus,
        error: Option<String>,
    },
    RawNotification {
        method: String,
        params: Value,
    },
}

#[async_trait]
pub trait AgentAdapter: Send + Sync {
    fn driver(&self) -> &'static str;
    fn capabilities(&self) -> AgentCapabilities;
    async fn run(&self, request: AgentRunRequest) -> Result<AgentStream, AdapterError>;
}
