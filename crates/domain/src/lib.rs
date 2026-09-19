//! Pure domain types and invariants for AIT.

/// Agent catalog entries, immutable revisions, capabilities, and tool policy.
pub mod agent;
/// Shared serialization-safe domain value objects.
pub mod common;
/// Scheduled Run configuration and policies.
pub mod cron;
/// Stable cross-layer error envelope and codes.
pub mod error;
/// Git identities shared by Project and Message boundaries.
pub mod git;
/// Immutable Project instruction snapshots.
pub mod instruction;
/// Shared execution lifecycle policy.
pub mod lifecycle;
/// Immutable Message protocol and projections.
pub mod message;
/// Run permission snapshots and native approval audit vocabulary.
pub mod permission;
/// Project registration, catalog state, and revisioned defaults.
pub mod project;
/// Run lifecycle, attempts, queue items, budgets, and usage.
pub mod run;
/// Session aggregate, identity, and atomic reference transitions.
pub mod session;
/// Tool execution lifecycle and audit links.
pub mod tool;

pub use agent::provider::{AgentProvider, ProviderKind, ProviderModel};
pub use agent::{
    Agent, AgentCapability, AgentConfigSnapshot, AgentConfiguration, AgentId, AgentRevision,
    ToolPermission, ToolPolicy,
};
pub use common::{CostMicros, DomainMetadata, DurationMs, TimestampMs};
pub use cron::{
    Cron, CronConcurrencyPolicy, CronFire, CronFireState, CronId, CronMisfirePolicy,
    cron_session_id,
};
pub use error::{DomainError, ErrorCode};
pub use git::GitCommit;
pub use instruction::{InstructionSnapshot, InstructionSourceSnapshot, InstructionSourceSummary};
pub use lifecycle::{LifecyclePhase, LifecycleStatus};
pub use message::{
    Message, MessageId, MessageKind, MessageOrigin, MessageRole, MessageValidationError,
    ProjectedMessage, ProviderItem, StoredMessage, SubMessage, SystemMessage,
    SystemMessageComponent, ToolResult, ToolResultStatus, ToolUse, message_path,
};
pub use permission::{
    ApprovalGrantScope, ApprovalMode, NativeApprovalFileChange, NativeApprovalFileChangeKind,
    NativeApprovalKind, NativeApprovalStatus, NativeApprovalTarget, NativeNetworkProtocol,
    RunPermissionProfile, SandboxAccess, ToolApprovalRecord, ToolApprovalState, ToolApprovalTarget,
    ToolGrant,
};
pub use project::{Project, ProjectDefaults, ProjectId, ProjectOwner, ProjectStatus};
pub use run::{
    CheckpointId, RetryPolicy, Run, RunAttempt, RunAttemptId, RunAttemptReason, RunAttemptStatus,
    RunBudget, RunId, RunPhase, RunQueueItem, RunQueueItemId, RunQueueItemKind, RunQueueItemStatus,
    RunStatus, RunStopReason, RunTerminationBlocker, RunTerminationReadiness, RunTrigger, RunUsage,
};
pub use session::{
    CodexThreadSource, CodexWorkspaceMode, CodexWriterState, ProviderHistoryCompleteness,
    ProviderRelationshipState, ProviderSyncState, Session, SessionId, SessionReference,
    SessionRoot, SessionSource, SessionStatus,
};
pub use tool::{ToolApprovalStatus, ToolExecution, ToolExecutionId, ToolExecutionStatus};
