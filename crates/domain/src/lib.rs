//! Pure domain types and invariants for AIT.

/// Agent catalog entries, immutable revisions, capabilities, and tool policy.
pub mod agent;
/// Shared serialization-safe domain value objects.
pub mod common;
/// Scheduled Run configuration and policies.
pub mod cron;
/// Stable cross-layer error envelope and codes.
pub mod error;
/// Immutable Message protocol and projections.
pub mod message;
/// Project instruction snapshots and movable Session references.
pub mod project;
/// Run lifecycle, attempts, queue items, budgets, and usage.
pub mod run;
/// Tool execution lifecycle and audit links.
pub mod tool;

pub use agent::{
    Agent, AgentCapability, AgentConfigSnapshot, AgentId, AgentRevision, ToolPermission, ToolPolicy,
};
pub use common::{CostMicros, DomainMetadata, DurationMs, TimestampMs};
pub use cron::{Cron, CronConcurrencyPolicy, CronFire, CronFireState, CronId, CronMisfirePolicy};
pub use error::{DomainError, ErrorCode};

pub use message::{
    Message, MessageKind, MessageOrigin, MessageRole, MessageValidationError, ProjectedMessage,
    RunId, StoredMessage, SubMessage, ToolResult, ToolResultStatus, ToolUse,
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
