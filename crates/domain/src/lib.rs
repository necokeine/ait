//! Pure domain types and invariants for AIT.

/// Agent catalog entries, immutable revisions, capabilities, and tool policy.
pub mod agent;
pub mod agent_provider;
/// Shared serialization-safe domain value objects.
pub mod common;
/// Scheduled Run configuration and policies.
pub mod cron;
/// Stable cross-layer error envelope and codes.
pub mod error;
/// Immutable Message protocol and projections.
pub mod message;
/// Immutable Message tree traversal policy.
pub mod message_path;
/// Run permission snapshots and native approval audit vocabulary.
pub mod permission;
/// Project instruction snapshots and movable Session references.
pub mod project;
/// Project registration and revisioned defaults.
pub mod project_policy;
pub use project_policy::ProjectDefaults;
/// Atomic Session pointer and binding rules.
pub mod session_reference;
pub use session_reference::SessionReference;
/// Shared execution lifecycle policy.
pub mod lifecycle;
/// Run lifecycle, attempts, queue items, budgets, and usage.
pub mod run;
pub use lifecycle::{LifecyclePhase, LifecycleStatus};
/// Tool execution lifecycle and audit links.
pub mod tool;

pub use agent::{
    Agent, AgentCapability, AgentConfigSnapshot, AgentId, AgentRevision, ToolPermission, ToolPolicy,
};
pub use agent_provider::{AgentConfiguration, AgentProvider, ProviderKind, ProviderModel};
pub use common::{CostMicros, DomainMetadata, DurationMs, TimestampMs};
pub use cron::{Cron, CronConcurrencyPolicy, CronFire, CronFireState, CronId, CronMisfirePolicy};
pub use error::{DomainError, ErrorCode};

pub use message::{
    Message, MessageKind, MessageOrigin, MessageRole, MessageValidationError, ProjectedMessage,
    RunId, StoredMessage, SubMessage, ToolResult, ToolResultStatus, ToolUse,
};
pub use permission::{
    ApprovalGrantScope, ApprovalMode, NativeApprovalFileChange, NativeApprovalFileChangeKind,
    NativeApprovalKind, NativeApprovalStatus, NativeApprovalTarget, NativeNetworkProtocol,
    RunPermissionProfile, SandboxAccess, ToolApprovalRecord, ToolApprovalState, ToolApprovalTarget,
    ToolGrant,
};
pub use project::{
    GitCommit, InstructionSnapshot, InstructionSourceSnapshot, InstructionSourceSummary, MessageId,
    Project, ProjectId, ProjectStatus, Session, SessionId, SessionRoot, SessionStatus,
    SystemMessage, SystemMessageComponent,
};
pub use run::{
    CheckpointId, RetryPolicy, Run, RunAttempt, RunAttemptId, RunAttemptReason, RunAttemptStatus,
    RunBudget, RunPhase, RunQueueItem, RunQueueItemId, RunQueueItemKind, RunQueueItemStatus,
    RunStatus, RunStopReason, RunTerminationBlocker, RunTerminationReadiness, RunTrigger, RunUsage,
};
pub use tool::{ToolApprovalStatus, ToolExecution, ToolExecutionId, ToolExecutionStatus};
