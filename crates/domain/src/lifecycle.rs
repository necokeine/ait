//! Application lifecycle vocabulary shared by API and native Workspace execution.
use serde::{Deserialize, Serialize};

/// Typed lifecycle status at the application lifecycle boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LifecycleStatus {
    /// queued.
    Queued,
    /// running.
    Running,
    /// waiting approval.
    WaitingApproval,
    /// retry wait.
    RetryWait,
    /// settling.
    Settling,
    /// completed.
    Completed,
    /// failed.
    Failed,
    /// cancelled.
    Cancelled,
    /// limit exceeded.
    LimitExceeded,
    /// interrupted.
    Interrupted,
    /// cancelling.
    Cancelling,
}
impl LifecycleStatus {
    /// Stable contract spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::WaitingApproval => "waiting_approval",
            Self::RetryWait => "retry_wait",
            Self::Settling => "settling",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::LimitExceeded => "limit_exceeded",
            Self::Interrupted => "interrupted",
            Self::Cancelling => "cancelling",
        }
    }
}
/// Typed lifecycle phase at the application lifecycle boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LifecyclePhase {
    /// queued.
    Queued,
    /// acquiring session ref.
    AcquiringSessionRef,
    /// assembling context.
    AssemblingContext,
    /// calling agent.
    CallingAgent,
    /// persisting message and advancing session.
    PersistingMessageAndAdvancingSession,
    /// waiting approval.
    WaitingApproval,
    /// executing tool.
    ExecutingTool,
    /// persisting tool result.
    PersistingToolResult,
    /// retry wait.
    RetryWait,
    /// compacting context.
    CompactingContext,
    /// checkpointing.
    Checkpointing,
    /// recovering.
    Recovering,
    /// draining queue.
    DrainingQueue,
    /// settling.
    Settling,
    /// releasing session ref.
    ReleasingSessionRef,
    /// terminal.
    Terminal,
    /// result persisted.
    ResultPersisted,
    /// integrating.
    Integrating,
    /// reconciling result.
    ReconcilingResult,
}
impl LifecyclePhase {
    /// Stable contract spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::AcquiringSessionRef => "acquiring_session_ref",
            Self::AssemblingContext => "assembling_context",
            Self::CallingAgent => "calling_agent",
            Self::PersistingMessageAndAdvancingSession => {
                "persisting_message_and_advancing_session"
            }
            Self::WaitingApproval => "waiting_approval",
            Self::ExecutingTool => "executing_tool",
            Self::PersistingToolResult => "persisting_tool_result",
            Self::RetryWait => "retry_wait",
            Self::CompactingContext => "compacting_context",
            Self::Checkpointing => "checkpointing",
            Self::Recovering => "recovering",
            Self::DrainingQueue => "draining_queue",
            Self::Settling => "settling",
            Self::ReleasingSessionRef => "releasing_session_ref",
            Self::Terminal => "terminal",
            Self::ResultPersisted => "result_persisted",
            Self::Integrating => "integrating",
            Self::ReconcilingResult => "reconciling_result",
        }
    }
}
impl LifecycleStatus {
    /// Whether no further execution may start.
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Completed
                | Self::Failed
                | Self::Cancelled
                | Self::LimitExceeded
                | Self::Interrupted
        )
    }
}
impl From<crate::RunStatus> for LifecycleStatus {
    fn from(status: crate::RunStatus) -> Self {
        match status {
            crate::RunStatus::Queued => Self::Queued,
            crate::RunStatus::Running => Self::Running,
            crate::RunStatus::WaitingApproval => Self::WaitingApproval,
            crate::RunStatus::RetryWait => Self::RetryWait,
            crate::RunStatus::Settling => Self::Settling,
            crate::RunStatus::Completed => Self::Completed,
            crate::RunStatus::Failed => Self::Failed,
            crate::RunStatus::Cancelled => Self::Cancelled,
            crate::RunStatus::LimitExceeded => Self::LimitExceeded,
        }
    }
}
impl From<crate::RunPhase> for LifecyclePhase {
    fn from(phase: crate::RunPhase) -> Self {
        match phase {
            crate::RunPhase::Queued => Self::Queued,
            crate::RunPhase::AcquiringSessionRef => Self::AcquiringSessionRef,
            crate::RunPhase::AssemblingContext => Self::AssemblingContext,
            crate::RunPhase::CallingAgent => Self::CallingAgent,
            crate::RunPhase::PersistingMessageAndAdvancingSession => {
                Self::PersistingMessageAndAdvancingSession
            }
            crate::RunPhase::WaitingApproval => Self::WaitingApproval,
            crate::RunPhase::ExecutingTool => Self::ExecutingTool,
            crate::RunPhase::PersistingToolResult => Self::PersistingToolResult,
            crate::RunPhase::RetryWait => Self::RetryWait,
            crate::RunPhase::CompactingContext => Self::CompactingContext,
            crate::RunPhase::Checkpointing => Self::Checkpointing,
            crate::RunPhase::Recovering => Self::Recovering,
            crate::RunPhase::DrainingQueue => Self::DrainingQueue,
            crate::RunPhase::Settling => Self::Settling,
            crate::RunPhase::ReleasingSessionRef => Self::ReleasingSessionRef,
            crate::RunPhase::Terminal => Self::Terminal,
        }
    }
}
