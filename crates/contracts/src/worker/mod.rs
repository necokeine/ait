//! Private, versioned daemon/worker protocol. Never a public client API.

pub mod codex;
pub mod model;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// Protocol value `PROTOCOL_MAJOR`.
pub const PROTOCOL_MAJOR: u16 = 2;
/// Protocol value `PROTOCOL_MINOR`.
pub const PROTOCOL_MINOR: u16 = 0;
/// Protocol value `MINIMUM_PROTOCOL_MINOR`.
pub const MINIMUM_PROTOCOL_MINOR: u16 = 0;
/// Protocol value `MAX_FRAME_BYTES`.
pub const MAX_FRAME_BYTES: u32 = 1_048_576;
/// Protocol value `REQUIRED_CAPABILITIES`.
pub const REQUIRED_CAPABILITIES: &[&str] = &[
    "run-store-v1",
    "commit-ack-v1",
    "lease-v1",
    "tool-grants-v1",
    "tool-interactions-v1",
    "native-codex-v1",
];
/// Protocol value `SUPPORTED_CAPABILITIES`.
pub const SUPPORTED_CAPABILITIES: &[&str] = REQUIRED_CAPABILITIES;

/// Errors deliberately carry no peer-controlled diagnostic strings.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ProtocolError {
    /// Selects the `InvalidFrame` variant.
    InvalidFrame,
    /// Selects the `FrameTooLarge` variant.
    FrameTooLarge,
    /// Selects the `UnexpectedEof` variant.
    UnexpectedEof,
    /// Selects the `Io` variant.
    Io,
    /// Selects the `VersionMismatch` variant.
    VersionMismatch,
    /// Selects the `UnsupportedCapability` variant.
    UnsupportedCapability,
    /// Selects the `SequenceRollback` variant.
    SequenceRollback,
    /// Selects the `CorrelationMismatch` variant.
    CorrelationMismatch,
    /// Selects the `StaleWorkerLease` variant.
    StaleWorkerLease,
    /// Selects the `WrongRun` variant.
    WrongRun,
    /// Selects the `InvalidTransition` variant.
    InvalidTransition,
    /// Selects the `OperationConflict` variant.
    OperationConflict,
    /// Selects the `HandshakeTimeout` variant.
    HandshakeTimeout,
    /// Selects the `HeartbeatTimeout` variant.
    HeartbeatTimeout,
    /// Selects the `WorkerExited` variant.
    WorkerExited,
    /// Selects the `ResourceLimit` variant.
    ResourceLimit,
    /// Selects the `Draining` variant.
    Draining,
}
impl std::fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "worker protocol: {self:?}")
    }
}
impl std::error::Error for ProtocolError {}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
/// Data carried by `Lease`.
pub struct Lease {
    /// Execution scope identity: an API Run or an independent Codex operation.
    pub scope_id: String,
    /// Worker instance identifier.
    pub worker_instance_id: String,
    /// Lease epoch value.
    pub lease_epoch: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
/// Data carried by `Hello`.
pub struct Hello {
    /// Protocol major value.
    pub protocol_major: u16,
    /// Highest wire minor this worker can emit and consume.
    pub protocol_minor: u16,
    /// Oldest wire minor this worker can consume.
    pub minimum_protocol_minor: u16,
    /// Optional and required capabilities implemented by this worker.
    pub capabilities: Vec<String>,
    /// Required capabilities value.
    pub required_capabilities: Vec<String>,
    /// Max frame bytes value.
    pub max_frame_bytes: u32,
    /// Pid value.
    pub pid: u32,
}
impl Hello {
    #[must_use]
    /// Builds the worker handshake advertised by this binary.
    pub fn current() -> Self {
        Self {
            protocol_major: PROTOCOL_MAJOR,
            protocol_minor: PROTOCOL_MINOR,
            minimum_protocol_minor: MINIMUM_PROTOCOL_MINOR,
            capabilities: SUPPORTED_CAPABILITIES
                .iter()
                .map(|capability| (*capability).into())
                .collect(),
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
        if self.minimum_protocol_minor > self.protocol_minor
            || self.minimum_protocol_minor > PROTOCOL_MINOR
        {
            return Err(ProtocolError::VersionMismatch);
        }
        if self
            .required_capabilities
            .iter()
            .any(|s| !REQUIRED_CAPABILITIES.contains(&s.as_str()))
            || REQUIRED_CAPABILITIES
                .iter()
                .any(|required| !self.capabilities.iter().any(|offered| offered == required))
        {
            return Err(ProtocolError::UnsupportedCapability);
        }
        if self.max_frame_bytes == 0 {
            return Err(ProtocolError::FrameTooLarge);
        }
        Ok(())
    }

    /// Select the newest mutually supported minor, bounded frame size, and
    /// capability intersection. Unknown optional capabilities are ignored.
    /// # Errors
    /// Rejects incompatible ranges, missing required capabilities, and invalid bounds.
    pub fn negotiate(&self, daemon_max_frame_bytes: u32) -> Result<HelloAck, ProtocolError> {
        self.validate()?;
        // Protocol 2 currently has only minor 0.
        let protocol_minor = PROTOCOL_MINOR;
        if protocol_minor < self.minimum_protocol_minor {
            return Err(ProtocolError::VersionMismatch);
        }
        let max_frame_bytes = self
            .max_frame_bytes
            .min(daemon_max_frame_bytes)
            .min(MAX_FRAME_BYTES);
        if max_frame_bytes == 0 {
            return Err(ProtocolError::FrameTooLarge);
        }
        let offered: BTreeSet<&str> = self.capabilities.iter().map(String::as_str).collect();
        let capabilities = SUPPORTED_CAPABILITIES
            .iter()
            .filter(|capability| offered.contains(**capability))
            .map(|capability| (*capability).to_owned())
            .collect();
        Ok(HelloAck {
            protocol_major: PROTOCOL_MAJOR,
            protocol_minor,
            max_frame_bytes,
            capabilities,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
/// Data carried by `HelloAck`.
pub struct HelloAck {
    /// Protocol major value.
    pub protocol_major: u16,
    /// Protocol minor value.
    pub protocol_minor: u16,
    /// Max frame bytes value.
    pub max_frame_bytes: u32,
    /// Capabilities value.
    pub capabilities: Vec<String>,
}
impl HelloAck {
    /// Validate the daemon's selection against the worker offer.
    /// # Errors
    /// Rejects any value outside the offered version, frame, or capability set.
    pub fn validate(&self, hello: &Hello) -> Result<(), ProtocolError> {
        if self.protocol_major != PROTOCOL_MAJOR
            || self.protocol_minor < hello.minimum_protocol_minor
            || self.protocol_minor > hello.protocol_minor
            || self.protocol_minor > PROTOCOL_MINOR
        {
            return Err(ProtocolError::VersionMismatch);
        }
        if self.max_frame_bytes == 0
            || self.max_frame_bytes > hello.max_frame_bytes
            || self.max_frame_bytes > MAX_FRAME_BYTES
        {
            return Err(ProtocolError::FrameTooLarge);
        }
        let offered: BTreeSet<&str> = hello.capabilities.iter().map(String::as_str).collect();
        let selected: BTreeSet<&str> = self.capabilities.iter().map(String::as_str).collect();
        if self.capabilities.len() != selected.len()
            || selected
                .iter()
                .any(|capability| !offered.contains(capability))
            || selected
                .iter()
                .any(|capability| !SUPPORTED_CAPABILITIES.contains(capability))
            || REQUIRED_CAPABILITIES
                .iter()
                .any(|required| !selected.contains(required))
        {
            return Err(ProtocolError::UnsupportedCapability);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
/// Data carried by `Limits`.
pub struct Limits {
    /// Max frame bytes value.
    pub max_frame_bytes: u32,
    /// Max output bytes value.
    pub max_output_bytes: u32,
    /// Max tool concurrency value.
    pub max_tool_concurrency: u16,
    /// Max steps value.
    pub max_steps: u64,
    /// Max tokens value.
    pub max_tokens: u64,
    /// An enabled monetary ceiling requires verifiable cost before a Provider call.
    pub max_cost_micros: Option<u64>,
    /// Wall clock ms value.
    pub wall_clock_ms: u64,
    /// Heartbeat ms value.
    pub heartbeat_ms: u64,
    /// Heartbeat timeout ms value.
    pub heartbeat_timeout_ms: u64,
    /// Drain ms value.
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
#[serde(tag = "kind", rename_all = "snake_case")]
/// Variants represented by `Executor`.
pub enum Executor {
    /// Native Codex operation owned entirely by this worker.
    Codex {
        /// Trusted executable selected by the daemon.
        binary: String,
        /// Operation with its own lifetime and correlation identity.
        operation: Box<codex::Operation>,
    },
    /// Selects the `Api` variant.
    Api {
        /// Provider value.
        provider: String,
        /// Endpoint value.
        endpoint: Option<String>,
        /// Model value.
        model: String,
        /// Reasoning effort value.
        reasoning_effort: Option<String>,
        /// Credential value.
        credential: CredentialGrant,
    },
    /// Selects the `Scripted` variant.
    Scripted {
        /// Replies value.
        replies: Vec<Vec<model::SubMessage>>,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
/// Data carried by `Bootstrap`.
pub struct Bootstrap {
    /// Lease value.
    pub lease: Lease,
    /// Limits value.
    pub limits: Limits,
    /// Workdir value.
    pub workdir: String,
    /// Permission value.
    pub permission: model::RunPermissionProfile,
    /// Maximum sandbox value.
    pub maximum_sandbox: model::SandboxAccess,
    /// Executor value.
    pub executor: Executor,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
/// Data carried by `ToolInteractionRequest`.
pub struct ToolInteractionRequest {
    /// Run identifier.
    pub run_id: String,
    /// Call identifier.
    pub call_id: String,
    /// Execution identifier.
    pub execution_id: String,
    /// Tool name value.
    pub tool_name: String,
    /// Arguments value.
    pub arguments: serde_json::Value,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "method", rename_all = "snake_case")]
/// Variants represented by `StoreRequest`.
pub enum StoreRequest {
    /// Ordered chunk of one serialized Codex result.
    CodexChunk {
        /// Byte offset within this result.
        offset: usize,
        /// Total serialized size.
        total: usize,
        /// Bounded bytes, never a diagnostic log.
        bytes: Vec<u8>,
    },
    /// Wait for the next application admission command.
    CodexNext,
    /// The owned app-server has been closed and reaped.
    CodexClosed,
    /// Selects the `WorkspaceProgress` variant.
    WorkspaceProgress {
        /// Event value.
        event: Box<model::WorkspaceProgressEvent>,
    },
    /// Selects the `WorkspaceApproval` variant.
    WorkspaceApproval {
        /// Request value.
        request: Box<model::WorkspaceApprovalRequest>,
        /// Expire value.
        expire: bool,
    },
    /// Selects the `LoadRun` variant.
    LoadRun,
    /// Selects the `MessagePath` variant.
    MessagePath {
        /// Head value.
        head: String,
        /// Offset value.
        offset: u32,
    },
    /// Selects the `Attempts` variant.
    Attempts {
        /// Offset value.
        offset: u32,
    },
    /// Selects the `Tools` variant.
    Tools {
        /// Assistant value.
        assistant: String,
        /// Offset value.
        offset: u32,
    },
    /// Selects the `SaveRun` variant.
    SaveRun {
        /// Run value.
        run: Box<model::Run>,
    },
    /// Selects the `SaveAttempt` variant.
    SaveAttempt {
        /// Run value.
        run: Box<model::Run>,
        /// Attempt value.
        attempt: model::RunAttempt,
    },
    /// Selects the `AppendMessage` variant.
    AppendMessage {
        /// Run value.
        run: Box<model::Run>,
        /// Message value.
        message: Box<model::Message>,
    },
    /// Selects the `SaveTool` variant.
    SaveTool {
        /// Run value.
        run: Box<model::Run>,
        /// Tool value.
        tool: Box<model::ToolExecution>,
    },
    /// Selects the `AppendToolResult` variant.
    AppendToolResult {
        /// Run value.
        run: Box<model::Run>,
        /// Tool value.
        tool: Box<model::ToolExecution>,
        /// Message value.
        message: Box<model::Message>,
    },
    /// Selects the `Complete` variant.
    Complete {
        /// Run value.
        run: Box<model::Run>,
        /// Queue version value.
        queue_version: u64,
    },
    /// Selects the `DrainQueue` variant.
    DrainQueue {
        /// Run value.
        run: Box<model::Run>,
    },
    /// Selects the `Approval` variant.
    Approval {
        /// Execution value.
        execution: Box<model::ToolExecution>,
    },
    /// Selects the `ConsumeToolGrant` variant.
    ConsumeToolGrant {
        /// Grant value.
        grant: Box<ait_domain::ToolGrant>,
    },
    /// Selects the `ToolInteraction` variant.
    ToolInteraction {
        /// Request value.
        request: Box<ToolInteractionRequest>,
    },
    /// Selects the `ToolInteractionRecovery` variant.
    ToolInteractionRecovery {
        /// Execution value.
        execution: Box<model::ToolExecution>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
/// Variants represented by `StoreResponse`.
pub enum StoreResponse {
    /// Application command for a prepared writer.
    CodexAction {
        /// Exactly one next action.
        action: codex::Action,
    },
    /// Selects the `Unit` variant.
    Unit,
    /// Selects the `WorkspaceApproval` variant.
    WorkspaceApproval {
        /// Decision value.
        decision: model::WorkspaceApprovalDecision,
    },
    /// Selects the `Run` variant.
    Run {
        /// Run value.
        run: Box<model::Run>,
    },
    /// Selects the `Messages` variant.
    Messages {
        /// Entries value.
        entries: Vec<model::ProjectedMessage>,
        /// Next value.
        next: Option<u32>,
    },
    /// Selects the `Attempts` variant.
    Attempts {
        /// Entries value.
        entries: Vec<model::RunAttempt>,
        /// Next value.
        next: Option<u32>,
    },
    /// Selects the `Tools` variant.
    Tools {
        /// Entries value.
        entries: Vec<model::ToolExecution>,
        /// Next value.
        next: Option<u32>,
    },
    /// Selects the `Completion` variant.
    Completion {
        /// Run value.
        run: Box<model::Run>,
        /// Completed value.
        completed: bool,
    },
    /// Selects the `Approval` variant.
    Approval {
        /// Decision value.
        decision: String,
    },
    /// Selects the `ToolGrant` variant.
    ToolGrant {
        /// Grant value.
        grant: Box<ait_domain::ToolGrant>,
    },
    /// Selects the `ToolInteraction` variant.
    ToolInteraction {
        /// Output value.
        output: serde_json::Value,
    },
    /// Selects the `ToolRecovery` variant.
    ToolRecovery {
        /// Recovery value.
        recovery: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        /// Output value.
        output: Option<serde_json::Value>,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
/// Variants represented by `Payload`.
pub enum Payload {
    /// Selects the `Hello` variant.
    Hello(Hello),
    /// Selects the `HelloAck` variant.
    HelloAck(HelloAck),
    /// Selects the `Bootstrap` variant.
    Bootstrap(Box<Bootstrap>),
    /// Selects the `Ready` variant.
    Ready {
        /// Pid value.
        pid: u32,
    },
    /// Selects the `Request` variant.
    Request {
        /// Request identifier.
        request_id: u64,
        /// Operation identifier.
        operation_id: String,
        /// Request value.
        request: Box<StoreRequest>,
    },
    /// Selects the `Receipt` variant.
    Receipt {
        /// Request identifier.
        request_id: u64,
        /// Operation identifier.
        operation_id: String,
        /// Response value.
        response: Box<StoreResponse>,
    },
    /// Selects the `Rejected` variant.
    Rejected {
        /// Request identifier.
        request_id: u64,
        /// Code value.
        code: ProtocolError,
    },
    /// Selects the `Heartbeat` variant.
    Heartbeat,
    /// Selects the `Cancel` variant.
    Cancel,
    /// Selects the `ExitReport` variant.
    ExitReport,
}

/// Every post-bootstrap frame is bound to the exact worker lease.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Envelope {
    /// Protocol major value.
    pub protocol_major: u16,
    /// Protocol minor value.
    pub protocol_minor: u16,
    /// Sequence value.
    pub sequence: u64,
    /// Lease value.
    pub lease: Option<Lease>,
    /// Payload value.
    pub payload: Payload,
}

#[cfg(test)]
mod tests;
