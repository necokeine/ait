//! Private, versioned daemon/worker protocol. Never a public client API.
#![allow(missing_docs)]

pub mod model;
use serde::{Deserialize, Serialize};

pub const PROTOCOL_MAJOR: u16 = 1;
pub const MAX_FRAME_BYTES: u32 = 1_048_576;
pub const REQUIRED_CAPABILITIES: &[&str] = &["run-store-v1", "commit-ack-v1", "lease-v1"];

/// Errors deliberately carry no peer-controlled diagnostic strings.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ProtocolError {
    InvalidFrame,
    FrameTooLarge,
    UnexpectedEof,
    Io,
    VersionMismatch,
    UnsupportedCapability,
    SequenceRollback,
    CorrelationMismatch,
    StaleWorkerLease,
    WrongRun,
    InvalidTransition,
    OperationConflict,
    HandshakeTimeout,
    HeartbeatTimeout,
    WorkerExited,
    ResourceLimit,
    Draining,
}
impl std::fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "worker protocol: {self:?}")
    }
}
impl std::error::Error for ProtocolError {}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Lease {
    pub run_id: String,
    pub worker_instance_id: String,
    pub lease_epoch: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Hello {
    pub protocol_major: u16,
    pub required_capabilities: Vec<String>,
    pub max_frame_bytes: u32,
    pub pid: u32,
}
impl Hello {
    #[must_use]
    pub fn current() -> Self {
        Self {
            protocol_major: PROTOCOL_MAJOR,
            required_capabilities: REQUIRED_CAPABILITIES.iter().map(|s| (*s).into()).collect(),
            max_frame_bytes: MAX_FRAME_BYTES,
            pid: std::process::id(),
        }
    }
    /// Validate version, required capabilities and frame bounds.
    /// # Errors
    /// Rejects any unsupported version/capability or invalid bound.
    pub fn validate(&self) -> Result<(), ProtocolError> {
        if self.protocol_major != PROTOCOL_MAJOR {
            return Err(ProtocolError::VersionMismatch);
        }
        if self
            .required_capabilities
            .iter()
            .any(|s| !REQUIRED_CAPABILITIES.contains(&s.as_str()))
            || REQUIRED_CAPABILITIES.iter().any(|required| {
                !self
                    .required_capabilities
                    .iter()
                    .any(|offered| offered == required)
            })
        {
            return Err(ProtocolError::UnsupportedCapability);
        }
        if self.max_frame_bytes == 0 || self.max_frame_bytes > MAX_FRAME_BYTES {
            return Err(ProtocolError::FrameTooLarge);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Limits {
    pub max_frame_bytes: u32,
    pub max_output_bytes: u32,
    pub max_tool_concurrency: u16,
    pub max_steps: u64,
    pub max_tokens: u64,
    /// An enabled monetary ceiling requires verifiable cost before a Provider call.
    pub max_cost_micros: Option<u64>,
    pub wall_clock_ms: u64,
    pub heartbeat_ms: u64,
    pub heartbeat_timeout_ms: u64,
    pub drain_ms: u64,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            max_frame_bytes: MAX_FRAME_BYTES,
            max_output_bytes: 65_536,
            max_tool_concurrency: 4,
            max_steps: 128,
            max_tokens: 1_000_000,
            max_cost_micros: None,
            wall_clock_ms: 300_000,
            heartbeat_ms: 500,
            heartbeat_timeout_ms: 3_000,
            drain_ms: 2_000,
        }
    }
}
impl Limits {
    /// Validate every requested limit against the compiled administrator ceiling.
    /// # Errors
    /// Rejects zero, inconsistent or excessive limits.
    pub fn validate(&self) -> Result<(), ProtocolError> {
        let ceiling = Self::default();
        if self.max_frame_bytes == 0
            || self.max_frame_bytes > ceiling.max_frame_bytes
            || self.max_output_bytes == 0
            || self.max_output_bytes > ceiling.max_output_bytes
            || self.max_tool_concurrency == 0
            || self.max_tool_concurrency > ceiling.max_tool_concurrency
            || self.max_steps == 0
            || self.max_steps > ceiling.max_steps
            || self.max_tokens == 0
            || self.max_tokens > ceiling.max_tokens
            || self.wall_clock_ms == 0
            || self.wall_clock_ms > ceiling.wall_clock_ms
            || self.heartbeat_ms == 0
            || self.heartbeat_timeout_ms <= self.heartbeat_ms
            || self.heartbeat_timeout_ms > ceiling.heartbeat_timeout_ms
            || self.drain_ms > ceiling.drain_ms
        {
            return Err(ProtocolError::ResourceLimit);
        }
        Ok(())
    }
}

/// Private grant: serializable only for the pipe, with intentionally redacted Debug.
#[derive(Clone, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CredentialGrant(pub String);
impl std::fmt::Debug for CredentialGrant {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("[REDACTED]")
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Executor {
    Api {
        provider: String,
        endpoint: Option<String>,
        model: String,
        reasoning_effort: Option<String>,
        credential: CredentialGrant,
    },
    Scripted {
        replies: Vec<Vec<model::SubMessage>>,
    },
    Workspace {
        invocation: Box<WorkspaceInvocation>,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceInvocation {
    pub codex_binary: String,
    pub request_id: String,
    pub model: String,
    pub reasoning_effort: Option<String>,
    pub project_instructions: Option<String>,
    pub prompt: String,
    pub commit_subject: String,
    pub baseline_commit: String,
    pub baseline_index_tree: String,
    pub recovery_result: Option<model::WorkspaceAgentResponse>,
    pub baseline_ref: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Bootstrap {
    pub lease: Lease,
    pub limits: Limits,
    pub workdir: String,
    pub permission: model::RunPermissionProfile,
    pub maximum_sandbox: model::SandboxAccess,
    pub executor: Executor,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "method", rename_all = "snake_case", deny_unknown_fields)]
pub enum StoreRequest {
    WorkspaceProgress {
        event: Box<model::WorkspaceProgressEvent>,
    },
    WorkspaceCheckpoint {
        result: Box<model::WorkspaceAgentResponse>,
    },
    WorkspaceIntegration,
    WorkspaceApproval {
        request: Box<model::WorkspaceApprovalRequest>,
        expire: bool,
    },
    WorkspaceFinished {
        result: Box<model::WorkspaceAgentResponse>,
    },
    LoadRun,
    MessagePath {
        head: String,
        offset: u32,
    },
    Attempts {
        offset: u32,
    },
    Tools {
        assistant: String,
        offset: u32,
    },
    SaveRun {
        run: Box<model::Run>,
    },
    SaveAttempt {
        run: Box<model::Run>,
        attempt: model::RunAttempt,
    },
    AppendMessage {
        run: Box<model::Run>,
        message: Box<model::Message>,
    },
    SaveTool {
        run: Box<model::Run>,
        tool: Box<model::ToolExecution>,
    },
    AppendToolResult {
        run: Box<model::Run>,
        tool: Box<model::ToolExecution>,
        message: Box<model::Message>,
    },
    Complete {
        run: Box<model::Run>,
        queue_version: u64,
    },
    DrainQueue {
        run: Box<model::Run>,
    },
    Approval {
        execution: Box<model::ToolExecution>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum StoreResponse {
    Unit,
    WorkspaceApproval {
        decision: model::WorkspaceApprovalDecision,
    },
    Run {
        run: Box<model::Run>,
    },
    Messages {
        entries: Vec<model::ProjectedMessage>,
        next: Option<u32>,
    },
    Attempts {
        entries: Vec<model::RunAttempt>,
        next: Option<u32>,
    },
    Tools {
        entries: Vec<model::ToolExecution>,
        next: Option<u32>,
    },
    Completion {
        run: Box<model::Run>,
        completed: bool,
    },
    Approval {
        decision: String,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Payload {
    Hello(Hello),
    HelloAck,
    Bootstrap(Box<Bootstrap>),
    Ready {
        pid: u32,
    },
    Request {
        request_id: u64,
        operation_id: String,
        request: Box<StoreRequest>,
    },
    Receipt {
        request_id: u64,
        operation_id: String,
        response: Box<StoreResponse>,
    },
    Rejected {
        request_id: u64,
        code: ProtocolError,
    },
    Heartbeat,
    Cancel,
    ExitReport,
}

/// Every post-bootstrap frame is bound to the exact worker lease.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Envelope {
    pub sequence: u64,
    pub lease: Option<Lease>,
    pub payload: Payload,
}
