//! Explicit field conversions. Domain values are never serialized onto the pipe.
#![allow(clippy::wildcard_imports)]
use ait_contracts::worker::{ProtocolError, model as w};
use ait_domain as d;
use std::collections::{BTreeMap, BTreeSet};
/// Converts a domain value to an independently versioned worker value.
pub trait Wire: Sized {
    /// Worker-owned value type.
    type Value;
    /// Copy into the wire contract.
    fn to_wire(&self) -> Self::Value;
    /// Validate primitive representations on entry from the wire.
    /// # Errors
    /// Rejects invalid identifiers.
    fn from_wire(value: Self::Value) -> Result<Self, ProtocolError>;
}
macro_rules! scalar { ($($t:ty),*) => { $(impl Wire for $t { type Value=Self; fn to_wire(&self)->Self {self.clone()} fn from_wire(v:Self)->Result<Self,ProtocolError>{Ok(v)} })* }; }
scalar!(String, u64, u32, i64, bool, serde_json::Value);
impl<T: Wire> Wire for Option<T> {
    type Value = Option<T::Value>;
    fn to_wire(&self) -> Self::Value {
        self.as_ref().map(Wire::to_wire)
    }
    fn from_wire(v: Self::Value) -> Result<Self, ProtocolError> {
        v.map(T::from_wire).transpose()
    }
}
impl<T: Wire> Wire for Vec<T> {
    type Value = Vec<T::Value>;
    fn to_wire(&self) -> Self::Value {
        self.iter().map(Wire::to_wire).collect()
    }
    fn from_wire(v: Self::Value) -> Result<Self, ProtocolError> {
        v.into_iter().map(T::from_wire).collect()
    }
}
impl<T: Wire + Ord> Wire for BTreeSet<T>
where
    T::Value: Ord,
{
    type Value = BTreeSet<T::Value>;
    fn to_wire(&self) -> Self::Value {
        self.iter().map(Wire::to_wire).collect()
    }
    fn from_wire(v: Self::Value) -> Result<Self, ProtocolError> {
        v.into_iter().map(T::from_wire).collect()
    }
}
impl<T: Wire> Wire for BTreeMap<String, T> {
    type Value = BTreeMap<String, T::Value>;
    fn to_wire(&self) -> Self::Value {
        self.iter().map(|(k, v)| (k.clone(), v.to_wire())).collect()
    }
    fn from_wire(v: Self::Value) -> Result<Self, ProtocolError> {
        v.into_iter()
            .map(|(k, v)| Ok((k, T::from_wire(v)?)))
            .collect()
    }
}
macro_rules! id { ($($t:ident),*) => { $(impl Wire for d::$t { type Value=String; fn to_wire(&self)->String {self.as_str().into()} fn from_wire(v:String)->Result<Self,ProtocolError>{if v.is_empty() || v.len()>256 {Err(ProtocolError::InvalidFrame)} else {Ok(Self::new(v))}} })* }; }
id!(
    RunId,
    RunAttemptId,
    ProjectId,
    AgentId,
    SessionId,
    CronId,
    CheckpointId,
    ToolExecutionId
);
impl Wire for d::MessageId {
    type Value = String;
    fn to_wire(&self) -> String {
        self.as_uuid().to_string()
    }
    fn from_wire(v: String) -> Result<Self, ProtocolError> {
        uuid::Uuid::parse_str(&v)
            .map(Self::new)
            .map_err(|_| ProtocolError::InvalidFrame)
    }
}
impl Wire for d::GitCommit {
    type Value = String;
    fn to_wire(&self) -> String {
        self.as_str().into()
    }
    fn from_wire(v: String) -> Result<Self, ProtocolError> {
        Self::parse(v).map_err(|_| ProtocolError::InvalidFrame)
    }
}
macro_rules! wrapper { ($($t:ident : $v:ty),*) => { $(impl Wire for d::$t { type Value=$v; fn to_wire(&self)->Self::Value{self.0.clone()} fn from_wire(v:Self::Value)->Result<Self,ProtocolError>{Ok(Self(v))} })* }; }
wrapper!(TimestampMs:i64,DurationMs:u64,CostMicros:u64,DomainMetadata:BTreeMap<String,serde_json::Value>);
macro_rules! record { ($t:ident {$($field:ident),* $(,)?}) => { impl Wire for d::$t {type Value=w::$t; fn to_wire(&self)->Self::Value {w::$t {$($field:self.$field.to_wire()),*}} fn from_wire(v:Self::Value)->Result<Self,ProtocolError>{Ok(Self {$($field:Wire::from_wire(v.$field)?),*})} } }; }
macro_rules! enumeration { ($t:ident {$($variant:ident),* $(,)?}) => { impl Wire for d::$t {type Value=w::$t; fn to_wire(&self)->Self::Value {match self {$(Self::$variant=>w::$t::$variant),*}} fn from_wire(v:Self::Value)->Result<Self,ProtocolError>{Ok(match v {$(w::$t::$variant=>Self::$variant),*})} } }; }
record!(Run {
    id,
    project_id,
    base_message_id,
    last_message_id,
    follow_session_id,
    agent_id,
    agent_revision,
    agent_snapshot,
    trigger,
    cron_id,
    scheduled_at,
    status,
    phase,
    stop_reason,
    error,
    step_count,
    budget,
    usage,
    attempt_count,
    compaction_count,
    retry_policy,
    next_retry_at,
    checkpoint_id,
    queue_version,
    queue_cursor,
    dedupe_key,
    started_at,
    ended_at,
    created_at
});
record!(RunAttempt {
    id,
    run_id,
    number,
    reason,
    checkpoint_id,
    status,
    error,
    started_at,
    ended_at
});
record!(RunBudget {
    max_steps,
    token_budget,
    cost_budget,
    max_runtime
});
record!(RunUsage {
    input_tokens,
    cached_input_tokens,
    output_tokens,
    tool_executions,
    cost
});
record!(RetryPolicy {
    max_attempts,
    initial_delay,
    max_delay
});
enumeration!(RunTrigger { Manual, Cron });
enumeration!(RunStatus {
    Queued,
    Running,
    WaitingApproval,
    RetryWait,
    Settling,
    Completed,
    Failed,
    Cancelled,
    LimitExceeded
});
enumeration!(RunPhase {
    Queued,
    AcquiringSessionRef,
    AssemblingContext,
    CallingAgent,
    PersistingMessageAndAdvancingSession,
    WaitingApproval,
    ExecutingTool,
    PersistingToolResult,
    RetryWait,
    CompactingContext,
    Checkpointing,
    Recovering,
    DrainingQueue,
    Settling,
    ReleasingSessionRef,
    Terminal
});
enumeration!(RunStopReason {
    Completed,
    Cancelled,
    StepLimit,
    TokenBudget,
    CostBudget,
    RuntimeLimit,
    Failed,
    RetryExhausted
});
enumeration!(RunAttemptReason {
    Initial,
    Retry,
    Recovery
});
enumeration!(RunAttemptStatus {
    Running,
    Completed,
    Failed,
    Cancelled
});
record!(Message {
    id,
    project_id,
    parent_message_id,
    role,
    kind,
    origin,
    sub_messages,
    created_by_session_id,
    run_id,
    run_seq,
    tool_result,
    git_commit,
    metadata,
    created_at
});
enumeration!(MessageRole {
    User,
    System,
    Assistant
});
enumeration!(MessageKind {
    Standard,
    ToolResult
});
enumeration!(MessageOrigin {
    Project,
    Human,
    Agent,
    Tool,
    Scheduler,
    System
});
record!(ToolResult {
    call_id,
    status,
    output,
    error
});
enumeration!(ToolResultStatus {
    Succeeded,
    Failed,
    Denied,
    Cancelled
});
record!(ToolUse {
    call_id,
    tool_name,
    arguments,
    provider_metadata
});
record!(ToolExecution {
    id,
    run_id,
    call_id,
    assistant_message_id,
    tool_use_index,
    tool_result_message_id,
    tool_name,
    arguments,
    attempt,
    approval_status,
    status,
    result,
    error,
    started_at,
    ended_at,
    created_at
});
enumeration!(ToolApprovalStatus {
    NotRequired,
    Pending,
    Approved,
    Denied
});
enumeration!(ToolExecutionStatus {
    Pending,
    Running,
    Succeeded,
    Failed,
    Denied,
    Cancelled
});
record!(AgentConfigSnapshot {
    agent_id,
    revision,
    driver_type,
    connection_name,
    model,
    endpoint,
    capabilities,
    default_parameters,
    tool_policy,
    config_digest
});
enumeration!(AgentCapability {
    Text,
    FileInput,
    StructuredOutput,
    ToolUse,
    ParallelToolUse,
    CheckpointRecovery
});
record!(ToolPolicy { default, tools });
enumeration!(ToolPermission {
    Allow,
    RequireApproval,
    Deny
});
record!(DomainError {
    code,
    message,
    retryable,
    details,
    cause_id
});
enumeration!(ErrorCode {
    ProjectPathNotFound,
    ProjectPathNotDirectory,
    ProjectPathAlreadyRegistered,
    ProjectPathAlreadyExists,
    ProjectDefaultDirectoryUnavailable,
    ProjectDirectoryCreationFailed,
    ProjectGitInitFailed,
    ProjectGitDirty,
    ProjectGitHeadUnavailable,
    ProjectWorkspaceBusy,
    ProjectPathOutOfScope,
    InvalidProject,
    SessionNotFound,
    SessionBusy,
    SessionPointerConflict,
    SessionMessageProjectMismatch,
    InvalidSession,
    MessageNotFound,
    MessageProjectMismatch,
    InvalidRootMessage,
    InvalidMessageRole,
    InvalidMessageId,
    InvalidSubmessageKind,
    InvalidMessageRunProvenance,
    MessageImmutable,
    ToolUseRequiresAssistant,
    ToolResultRequiresUser,
    ToolResultMessageInvalid,
    HumanMessageGitCommitRequired,
    MessageGitCommitNotAllowed,
    InvalidMessageGitCommit,
    AgentNotFound,
    AgentDisabled,
    AgentRevisionNotFound,
    AgentCapabilityUnsupported,
    InvalidAgentConfiguration,
    InvalidConfiguration,
    ProviderFailed,
    InvalidRun,
    RunNotResumable,
    RunAlreadyTerminal,
    RunRetryExhausted,
    RunRecoveryFailed,
    RunQueueConflict,
    RunCancelled,
    RunLimitExceeded,
    ToolUseNotFound,
    ToolCallDuplicate,
    ToolResultDuplicate,
    ToolRunMismatch,
    ToolApprovalRequired,
    ToolExecutionFailed,
    InvalidToolExecution,
    CronProjectUnavailable,
    CronBaseMessageUnavailable,
    CronAgentUnavailable,
    CronDuplicateFire,
    CronConcurrencyBlocked,
    InvalidCron
});
record!(RunPermissionProfile { sandbox, approval });
enumeration!(SandboxAccess {
    ReadOnly,
    WorkspaceWrite,
    FullAccess
});
enumeration!(ApprovalMode {
    OnRequest,
    UntrustedOnly
});
impl Wire for d::SubMessage {
    type Value = w::SubMessage;
    fn to_wire(&self) -> Self::Value {
        match self {
            Self::Text { text } => w::SubMessage::Text {
                text: text.to_wire(),
            },
            Self::FileRef {
                attachment_id,
                media_type,
                name,
            } => w::SubMessage::FileRef {
                attachment_id: attachment_id.to_wire(),
                media_type: media_type.to_wire(),
                name: name.to_wire(),
            },
            Self::ToolUse(inner) => w::SubMessage::ToolUse(inner.to_wire()),
            Self::StructuredData { media_type, value } => w::SubMessage::StructuredData {
                media_type: media_type.to_wire(),
                value: value.to_wire(),
            },
        }
    }
    fn from_wire(value: Self::Value) -> Result<Self, ProtocolError> {
        Ok(match value {
            w::SubMessage::Text { text } => Self::Text {
                text: Wire::from_wire(text)?,
            },
            w::SubMessage::FileRef {
                attachment_id,
                media_type,
                name,
            } => Self::FileRef {
                attachment_id: Wire::from_wire(attachment_id)?,
                media_type: Wire::from_wire(media_type)?,
                name: Wire::from_wire(name)?,
            },
            w::SubMessage::ToolUse(inner) => Self::ToolUse(Wire::from_wire(inner)?),
            w::SubMessage::StructuredData { media_type, value } => Self::StructuredData {
                media_type: Wire::from_wire(media_type)?,
                value: Wire::from_wire(value)?,
            },
        })
    }
}
impl Wire for d::ProjectedMessage {
    type Value = w::ProjectedMessage;
    fn to_wire(&self) -> Self::Value {
        match self {
            Self::Visible(inner) => w::ProjectedMessage::Visible(inner.to_wire()),
            Self::Redacted {
                id,
                project_id,
                parent_message_id,
                role,
            } => w::ProjectedMessage::Redacted {
                id: id.to_wire(),
                project_id: project_id.to_wire(),
                parent_message_id: parent_message_id.to_wire(),
                role: role.to_wire(),
            },
        }
    }
    fn from_wire(value: Self::Value) -> Result<Self, ProtocolError> {
        Ok(match value {
            w::ProjectedMessage::Visible(inner) => Self::Visible(Wire::from_wire(inner)?),
            w::ProjectedMessage::Redacted {
                id,
                project_id,
                parent_message_id,
                role,
            } => Self::Redacted {
                id: Wire::from_wire(id)?,
                project_id: Wire::from_wire(project_id)?,
                parent_message_id: Wire::from_wire(parent_message_id)?,
                role: Wire::from_wire(role)?,
            },
        })
    }
}

impl Wire for ait_domain::ApprovalGrantScope {
    type Value = w::ApprovalGrantScope;
    fn to_wire(&self) -> Self::Value {
        match self {
            Self::OneShot => w::ApprovalGrantScope::OneShot,
            Self::Turn => w::ApprovalGrantScope::Turn,
            Self::Session => w::ApprovalGrantScope::Session,
        }
    }
    fn from_wire(value: Self::Value) -> Result<Self, ProtocolError> {
        Ok(match value {
            w::ApprovalGrantScope::OneShot => Self::OneShot,
            w::ApprovalGrantScope::Turn => Self::Turn,
            w::ApprovalGrantScope::Session => Self::Session,
        })
    }
}

impl Wire for ait_domain::NativeApprovalKind {
    type Value = w::NativeApprovalKind;
    fn to_wire(&self) -> Self::Value {
        match self {
            Self::CommandExecution => w::NativeApprovalKind::CommandExecution,
            Self::FileChange => w::NativeApprovalKind::FileChange,
            Self::Permissions => w::NativeApprovalKind::Permissions,
            Self::LegacyCommand => w::NativeApprovalKind::LegacyCommand,
            Self::LegacyPatch => w::NativeApprovalKind::LegacyPatch,
        }
    }
    fn from_wire(value: Self::Value) -> Result<Self, ProtocolError> {
        Ok(match value {
            w::NativeApprovalKind::CommandExecution => Self::CommandExecution,
            w::NativeApprovalKind::FileChange => Self::FileChange,
            w::NativeApprovalKind::Permissions => Self::Permissions,
            w::NativeApprovalKind::LegacyCommand => Self::LegacyCommand,
            w::NativeApprovalKind::LegacyPatch => Self::LegacyPatch,
        })
    }
}

impl Wire for ait_domain::NativeApprovalTarget {
    type Value = w::NativeApprovalTarget;
    fn to_wire(&self) -> Self::Value {
        match self {
            Self::Command { command, cwd } => w::NativeApprovalTarget::Command {
                command: command.to_wire(),
                cwd: cwd.to_wire(),
            },
            Self::Network { host, protocol } => w::NativeApprovalTarget::Network {
                host: host.to_wire(),
                protocol: protocol.to_wire(),
            },
            Self::FileChange {
                grant_root,
                changes,
            } => w::NativeApprovalTarget::FileChange {
                grant_root: grant_root.to_wire(),
                changes: changes.to_wire(),
            },
            Self::Permissions { cwd } => {
                w::NativeApprovalTarget::Permissions { cwd: cwd.to_wire() }
            }
        }
    }
    fn from_wire(value: Self::Value) -> Result<Self, ProtocolError> {
        Ok(match value {
            w::NativeApprovalTarget::Command { command, cwd } => Self::Command {
                command: Wire::from_wire(command)?,
                cwd: Wire::from_wire(cwd)?,
            },
            w::NativeApprovalTarget::Network { host, protocol } => Self::Network {
                host: Wire::from_wire(host)?,
                protocol: Wire::from_wire(protocol)?,
            },
            w::NativeApprovalTarget::FileChange {
                grant_root,
                changes,
            } => Self::FileChange {
                grant_root: Wire::from_wire(grant_root)?,
                changes: Wire::from_wire(changes)?,
            },
            w::NativeApprovalTarget::Permissions { cwd } => Self::Permissions {
                cwd: Wire::from_wire(cwd)?,
            },
        })
    }
}

impl Wire for ait_domain::NativeNetworkProtocol {
    type Value = w::NativeNetworkProtocol;
    fn to_wire(&self) -> Self::Value {
        match self {
            Self::Http => w::NativeNetworkProtocol::Http,
            Self::Https => w::NativeNetworkProtocol::Https,
            Self::Socks5Tcp => w::NativeNetworkProtocol::Socks5Tcp,
            Self::Socks5Udp => w::NativeNetworkProtocol::Socks5Udp,
        }
    }
    fn from_wire(value: Self::Value) -> Result<Self, ProtocolError> {
        Ok(match value {
            w::NativeNetworkProtocol::Http => Self::Http,
            w::NativeNetworkProtocol::Https => Self::Https,
            w::NativeNetworkProtocol::Socks5Tcp => Self::Socks5Tcp,
            w::NativeNetworkProtocol::Socks5Udp => Self::Socks5Udp,
        })
    }
}

impl Wire for ait_domain::NativeApprovalFileChange {
    type Value = w::NativeApprovalFileChange;
    fn to_wire(&self) -> Self::Value {
        w::NativeApprovalFileChange {
            path: self.path.to_wire(),
            kind: self.kind.to_wire(),
        }
    }
    fn from_wire(v: Self::Value) -> Result<Self, ProtocolError> {
        Ok(Self {
            path: Wire::from_wire(v.path)?,
            kind: Wire::from_wire(v.kind)?,
        })
    }
}

impl Wire for ait_ports::WorkspaceAgentResponse {
    type Value = w::WorkspaceAgentResponse;
    fn to_wire(&self) -> Self::Value {
        w::WorkspaceAgentResponse {
            assistant_text: self.assistant_text.to_wire(),
            commit_id: self.commit_id.to_wire(),
            operations: self.operations.to_wire(),
            output_items: self.output_items.to_wire(),
        }
    }
    fn from_wire(v: Self::Value) -> Result<Self, ProtocolError> {
        Ok(Self {
            assistant_text: Wire::from_wire(v.assistant_text)?,
            commit_id: Wire::from_wire(v.commit_id)?,
            operations: Wire::from_wire(v.operations)?,
            output_items: Wire::from_wire(v.output_items)?,
        })
    }
}

impl Wire for ait_ports::WorkspaceOperation {
    type Value = w::WorkspaceOperation;
    fn to_wire(&self) -> Self::Value {
        w::WorkspaceOperation {
            id: self.id.to_wire(),
            kind: self.kind.to_wire(),
            status: self.status.to_wire(),
            title: self.title.to_wire(),
            summary: self.summary.to_wire(),
            detail: self.detail.to_wire(),
            paths: self.paths.to_wire(),
        }
    }
    fn from_wire(v: Self::Value) -> Result<Self, ProtocolError> {
        Ok(Self {
            id: Wire::from_wire(v.id)?,
            kind: Wire::from_wire(v.kind)?,
            status: Wire::from_wire(v.status)?,
            title: Wire::from_wire(v.title)?,
            summary: Wire::from_wire(v.summary)?,
            detail: Wire::from_wire(v.detail)?,
            paths: Wire::from_wire(v.paths)?,
        })
    }
}

impl Wire for ait_ports::WorkspaceOutputItem {
    type Value = w::WorkspaceOutputItem;
    fn to_wire(&self) -> Self::Value {
        match self {
            Self::Message { id, phase, text } => w::WorkspaceOutputItem::Message {
                id: id.to_wire(),
                phase: phase.to_wire(),
                text: text.to_wire(),
            },
            Self::Operation { id } => w::WorkspaceOutputItem::Operation { id: id.to_wire() },
        }
    }
    fn from_wire(value: Self::Value) -> Result<Self, ProtocolError> {
        Ok(match value {
            w::WorkspaceOutputItem::Message { id, phase, text } => Self::Message {
                id: Wire::from_wire(id)?,
                phase: Wire::from_wire(phase)?,
                text: Wire::from_wire(text)?,
            },
            w::WorkspaceOutputItem::Operation { id } => Self::Operation {
                id: Wire::from_wire(id)?,
            },
        })
    }
}

impl Wire for ait_ports::WorkspaceProgressEvent {
    type Value = w::WorkspaceProgressEvent;
    fn to_wire(&self) -> Self::Value {
        match self {
            Self::MessageStarted { id, phase, text } => w::WorkspaceProgressEvent::MessageStarted {
                id: id.to_wire(),
                phase: phase.to_wire(),
                text: text.to_wire(),
            },
            Self::TextDelta { id, delta } => w::WorkspaceProgressEvent::TextDelta {
                id: id.to_wire(),
                delta: delta.to_wire(),
            },
            Self::MessageCompleted { id, phase, text } => {
                w::WorkspaceProgressEvent::MessageCompleted {
                    id: id.to_wire(),
                    phase: phase.to_wire(),
                    text: text.to_wire(),
                }
            }
            Self::OperationStarted(inner) => {
                w::WorkspaceProgressEvent::OperationStarted(inner.to_wire())
            }
            Self::OperationCompleted(inner) => {
                w::WorkspaceProgressEvent::OperationCompleted(inner.to_wire())
            }
            Self::Warning {
                message,
                retrying,
                code,
            } => w::WorkspaceProgressEvent::Warning {
                message: message.to_wire(),
                retrying: retrying.to_wire(),
                code: code.to_wire(),
            },
            Self::TurnStatus { status, error } => w::WorkspaceProgressEvent::TurnStatus {
                status: status.to_wire(),
                error: error.to_wire(),
            },
        }
    }
    fn from_wire(value: Self::Value) -> Result<Self, ProtocolError> {
        Ok(match value {
            w::WorkspaceProgressEvent::MessageStarted { id, phase, text } => Self::MessageStarted {
                id: Wire::from_wire(id)?,
                phase: Wire::from_wire(phase)?,
                text: Wire::from_wire(text)?,
            },
            w::WorkspaceProgressEvent::TextDelta { id, delta } => Self::TextDelta {
                id: Wire::from_wire(id)?,
                delta: Wire::from_wire(delta)?,
            },
            w::WorkspaceProgressEvent::MessageCompleted { id, phase, text } => {
                Self::MessageCompleted {
                    id: Wire::from_wire(id)?,
                    phase: Wire::from_wire(phase)?,
                    text: Wire::from_wire(text)?,
                }
            }
            w::WorkspaceProgressEvent::OperationStarted(inner) => {
                Self::OperationStarted(Wire::from_wire(inner)?)
            }
            w::WorkspaceProgressEvent::OperationCompleted(inner) => {
                Self::OperationCompleted(Wire::from_wire(inner)?)
            }
            w::WorkspaceProgressEvent::Warning {
                message,
                retrying,
                code,
            } => Self::Warning {
                message: Wire::from_wire(message)?,
                retrying: Wire::from_wire(retrying)?,
                code: Wire::from_wire(code)?,
            },
            w::WorkspaceProgressEvent::TurnStatus { status, error } => Self::TurnStatus {
                status: Wire::from_wire(status)?,
                error: Wire::from_wire(error)?,
            },
        })
    }
}

impl Wire for ait_ports::WorkspaceApprovalRequest {
    type Value = w::WorkspaceApprovalRequest;
    fn to_wire(&self) -> Self::Value {
        w::WorkspaceApprovalRequest {
            run_id: self.run_id.to_wire(),
            protocol_request_id: self.protocol_request_id.to_wire(),
            method: self.method.to_wire(),
            kind: self.kind.to_wire(),
            thread_id: self.thread_id.to_wire(),
            turn_id: self.turn_id.to_wire(),
            item_id: self.item_id.to_wire(),
            target: self.target.to_wire(),
            requested_permissions: self.requested_permissions.to_wire(),
        }
    }
    fn from_wire(v: Self::Value) -> Result<Self, ProtocolError> {
        Ok(Self {
            run_id: Wire::from_wire(v.run_id)?,
            protocol_request_id: Wire::from_wire(v.protocol_request_id)?,
            method: Wire::from_wire(v.method)?,
            kind: Wire::from_wire(v.kind)?,
            thread_id: Wire::from_wire(v.thread_id)?,
            turn_id: Wire::from_wire(v.turn_id)?,
            item_id: Wire::from_wire(v.item_id)?,
            target: Wire::from_wire(v.target)?,
            requested_permissions: Wire::from_wire(v.requested_permissions)?,
        })
    }
}

impl Wire for ait_ports::WorkspaceApprovalDecision {
    type Value = w::WorkspaceApprovalDecision;
    fn to_wire(&self) -> Self::Value {
        match self {
            Self::Approved { scope, permissions } => w::WorkspaceApprovalDecision::Approved {
                scope: scope.to_wire(),
                permissions: permissions.to_wire(),
            },
            Self::Denied => w::WorkspaceApprovalDecision::Denied,
            Self::Cancelled => w::WorkspaceApprovalDecision::Cancelled,
        }
    }
    fn from_wire(value: Self::Value) -> Result<Self, ProtocolError> {
        Ok(match value {
            w::WorkspaceApprovalDecision::Approved { scope, permissions } => Self::Approved {
                scope: Wire::from_wire(scope)?,
                permissions: Wire::from_wire(permissions)?,
            },
            w::WorkspaceApprovalDecision::Denied => Self::Denied,
            w::WorkspaceApprovalDecision::Cancelled => Self::Cancelled,
        })
    }
}
impl Wire for d::NativeApprovalFileChangeKind {
    type Value = w::NativeApprovalFileChangeKind;
    fn to_wire(&self) -> Self::Value {
        match self {
            Self::Add => w::NativeApprovalFileChangeKind::Add,
            Self::Delete => w::NativeApprovalFileChangeKind::Delete,
            Self::Update => w::NativeApprovalFileChangeKind::Update,
        }
    }
    fn from_wire(v: Self::Value) -> Result<Self, ProtocolError> {
        Ok(match v {
            w::NativeApprovalFileChangeKind::Add => Self::Add,
            w::NativeApprovalFileChangeKind::Delete => Self::Delete,
            w::NativeApprovalFileChangeKind::Update => Self::Update,
        })
    }
}
