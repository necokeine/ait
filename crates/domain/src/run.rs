use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    AgentConfigSnapshot, AgentId, CostMicros, CronId, DomainError, DurationMs, ErrorCode,
    MessageId, ProjectId, RunId, SessionId, TimestampMs,
};

/// Stable identity of one underlying Run attempt.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RunAttemptId(String);

impl RunAttemptId {
    /// Creates an externally assigned attempt identity.
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Returns the string representation.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Stable identity of one queued work item.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RunQueueItemId(String);

impl RunQueueItemId {
    /// Creates an externally assigned queue-item identity.
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Returns the string representation.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Stable identity of a persisted recovery checkpoint.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CheckpointId(String);

impl CheckpointId {
    /// Creates an externally assigned checkpoint identity.
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Returns the string representation.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Source that created a Run.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunTrigger {
    /// Explicit interactive or background request.
    Manual,
    /// One scheduled Cron occurrence.
    Cron,
}

/// Durable Run lifecycle state.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
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

impl RunStatus {
    /// Returns whether no more work may be enqueued into this Run.
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Failed | Self::Cancelled | Self::LimitExceeded
        )
    }
}

/// Fine-grained phase within the Run lifecycle.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
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

/// Stable reason a Run stopped.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
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

/// Limits fixed when a Run is created.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RunBudget {
    /// Maximum persisted Agent/tool steps; must be positive.
    pub max_steps: u64,
    /// Optional total token allowance.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_budget: Option<u64>,
    /// Optional total cost allowance.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_budget: Option<CostMicros>,
    /// Optional wall-clock allowance.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_runtime: Option<DurationMs>,
}

impl RunBudget {
    /// Validates that the mandatory step allowance is non-zero.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorCode::InvalidRun`] when `max_steps` is zero.
    pub fn validate(&self) -> Result<(), DomainError> {
        if self.max_steps == 0 {
            return Err(DomainError::invariant(
                ErrorCode::InvalidRun,
                "run max_steps must be positive",
            ));
        }
        Ok(())
    }
}

/// Provider-neutral usage accumulated across every attempt in a Run.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
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
    pub cost: Option<CostMicros>,
}

impl RunUsage {
    /// Returns total model tokens, saturating on overflow.
    #[must_use]
    pub const fn total_tokens(&self) -> u64 {
        self.input_tokens
            .saturating_add(self.cached_input_tokens)
            .saturating_add(self.output_tokens)
    }
}

/// Retry policy fixed for the lifetime of a Run.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RetryPolicy {
    /// Maximum number of attempts including the initial attempt.
    pub max_attempts: u32,
    /// Initial delay before retrying.
    pub initial_delay: DurationMs,
    /// Maximum delay after backoff.
    pub max_delay: DurationMs,
}

impl RetryPolicy {
    /// Validates attempt and delay bounds.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorCode::InvalidRun`] when no attempt is allowed or the
    /// maximum delay is shorter than the initial delay.
    pub fn validate(&self) -> Result<(), DomainError> {
        if self.max_attempts == 0 || self.max_delay < self.initial_delay {
            return Err(DomainError::invariant(
                ErrorCode::InvalidRun,
                "retry policy attempt or delay bounds are invalid",
            ));
        }
        Ok(())
    }
}

/// One complete task execution from a fixed Message and Agent revision.
///
/// Attempts, compaction recovery, and queued work remain inside this identity.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Run {
    /// Run identity.
    pub id: RunId,
    /// Owning Project.
    pub project_id: ProjectId,
    /// Immutable starting Message.
    pub base_message_id: MessageId,
    /// Last Message persisted by this Run, when any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_message_id: Option<MessageId>,
    /// Session advanced by outputs, absent for Cron/background Runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub follow_session_id: Option<SessionId>,
    /// Fixed Agent identity.
    pub agent_id: AgentId,
    /// Fixed Agent revision.
    pub agent_revision: u64,
    /// Reproducible non-secret copy of that exact revision.
    pub agent_snapshot: AgentConfigSnapshot,
    /// Trigger class.
    pub trigger: RunTrigger,
    /// Source Cron for a scheduled Run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cron_id: Option<CronId>,
    /// Scheduled occurrence for a Cron Run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scheduled_at: Option<TimestampMs>,
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
    pub next_retry_at: Option<TimestampMs>,
    /// Latest durable recovery checkpoint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checkpoint_id: Option<CheckpointId>,
    /// Version incremented whenever work is enqueued.
    pub queue_version: u64,
    /// Last consumed queue sequence.
    pub queue_cursor: u64,
    /// Optional idempotency key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dedupe_key: Option<String>,
    /// First execution time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<TimestampMs>,
    /// Terminal time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<TimestampMs>,
    /// Creation time.
    pub created_at: TimestampMs,
}

impl Run {
    /// Validates fixed references, trigger shape, budgets, and lifecycle fields.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorCode::InvalidRun`] for an impossible aggregate.
    pub fn validate(&self) -> Result<(), DomainError> {
        self.budget.validate()?;
        self.retry_policy.validate()?;

        let trigger_valid = match self.trigger {
            RunTrigger::Manual => self.cron_id.is_none() && self.scheduled_at.is_none(),
            // Legacy Cron Runs can be Sessionless; current occurrences create
            // and follow a fresh Session without invalidating durable history.
            RunTrigger::Cron => self.cron_id.is_some() && self.scheduled_at.is_some(),
        };
        let terminal_fields_valid = if self.status.is_terminal() {
            self.ended_at.is_some()
                && self.stop_reason.is_some()
                && self.phase == RunPhase::Terminal
        } else {
            self.ended_at.is_none()
                && self.stop_reason.is_none()
                && self.phase != RunPhase::Terminal
        };
        let status_phase_valid = match self.status {
            RunStatus::Queued => self.phase == RunPhase::Queued,
            RunStatus::WaitingApproval => self.phase == RunPhase::WaitingApproval,
            RunStatus::RetryWait => self.phase == RunPhase::RetryWait,
            RunStatus::Settling => self.phase == RunPhase::Settling,
            RunStatus::Completed
            | RunStatus::Failed
            | RunStatus::Cancelled
            | RunStatus::LimitExceeded => self.phase == RunPhase::Terminal,
            RunStatus::Running => !matches!(
                self.phase,
                RunPhase::Queued
                    | RunPhase::WaitingApproval
                    | RunPhase::RetryWait
                    | RunPhase::Settling
                    | RunPhase::Terminal
            ),
        };
        let stop_reason_valid = match self.status {
            RunStatus::Completed => self.stop_reason == Some(RunStopReason::Completed),
            RunStatus::Failed => matches!(
                self.stop_reason,
                Some(RunStopReason::Failed | RunStopReason::RetryExhausted)
            ),
            RunStatus::Cancelled => self.stop_reason == Some(RunStopReason::Cancelled),
            RunStatus::LimitExceeded => matches!(
                self.stop_reason,
                Some(
                    RunStopReason::StepLimit
                        | RunStopReason::TokenBudget
                        | RunStopReason::CostBudget
                        | RunStopReason::RuntimeLimit
                )
            ),
            _ => self.stop_reason.is_none(),
        };
        let retry_fields_valid =
            (self.status == RunStatus::RetryWait) == self.next_retry_at.is_some();
        let timestamps_valid = self.started_at.is_none_or(|time| time >= self.created_at)
            && self
                .ended_at
                .is_none_or(|time| time >= self.started_at.unwrap_or(self.created_at));
        let snapshot_valid = self.agent_revision > 0
            && self.agent_snapshot.agent_id == self.agent_id
            && self.agent_snapshot.revision == self.agent_revision;

        if self.id.as_str().is_empty()
            || self.project_id.as_str().is_empty()
            || self.base_message_id.as_uuid().is_nil()
            || !snapshot_valid
            || !trigger_valid
            || !terminal_fields_valid
            || !status_phase_valid
            || !stop_reason_valid
            || !retry_fields_valid
            || !timestamps_valid
            || self.step_count > self.budget.max_steps
            || self.queue_cursor > self.queue_version
        {
            return Err(DomainError::invariant(
                ErrorCode::InvalidRun,
                "run fixed references, lifecycle, counters, or timestamps are inconsistent",
            ));
        }
        Ok(())
    }
}

/// Why a low-level Agent invocation was started inside a Run.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunAttemptReason {
    /// First invocation.
    Initial,
    /// Retry after a recoverable failure.
    Retry,
    /// Process or context recovery.
    Recovery,
}

/// Lifecycle of one low-level Agent attempt.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
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

impl RunAttemptStatus {
    const fn is_terminal(self) -> bool {
        !matches!(self, Self::Running)
    }
}

/// Auditable low-level invocation inside a Run.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RunAttempt {
    /// Attempt identity.
    pub id: RunAttemptId,
    /// Owning Run.
    pub run_id: RunId,
    /// Monotonic Run-local number beginning at one.
    pub number: u32,
    /// Why this attempt started.
    pub reason: RunAttemptReason,
    /// Recovery checkpoint used by this attempt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checkpoint_id: Option<CheckpointId>,
    /// Attempt lifecycle state.
    pub status: RunAttemptStatus,
    /// Safe failure information.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<DomainError>,
    /// Start time.
    pub started_at: TimestampMs,
    /// End time for a terminal attempt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<TimestampMs>,
}

impl RunAttempt {
    /// Validates numbering, recovery, and terminal timestamps.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorCode::InvalidRun`] for an impossible attempt.
    pub fn validate(&self) -> Result<(), DomainError> {
        let reason_valid = match self.reason {
            RunAttemptReason::Initial => self.number == 1 && self.checkpoint_id.is_none(),
            RunAttemptReason::Retry => self.number > 1,
            RunAttemptReason::Recovery => self.number > 1 && self.checkpoint_id.is_some(),
        };
        if self.id.as_str().is_empty()
            || self.run_id.as_str().is_empty()
            || !reason_valid
            || self.status.is_terminal() != self.ended_at.is_some()
            || self.ended_at.is_some_and(|time| time < self.started_at)
        {
            return Err(DomainError::invariant(
                ErrorCode::InvalidRun,
                "run attempt numbering, recovery, state, or timestamps are inconsistent",
            ));
        }
        Ok(())
    }
}

/// Extensible, stable queue item kind.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RunQueueItemKind(String);

impl RunQueueItemKind {
    /// Creates a queue kind such as `user_input`.
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Returns the stable string representation.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Lifecycle of queued work belonging to a Run.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunQueueItemStatus {
    /// Waiting to be claimed in sequence order.
    Pending,
    /// Currently being converted into Message work.
    Processing,
    /// Successfully consumed.
    Consumed,
    /// Permanently rejected.
    Rejected,
}

/// Work appended to an already active Run.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RunQueueItem {
    /// Queue item identity.
    pub id: RunQueueItemId,
    /// Owning Run.
    pub run_id: RunId,
    /// Monotonic Run-local sequence beginning at one.
    pub sequence: u64,
    /// Extensible work kind.
    pub kind: RunQueueItemKind,
    /// Structured, non-secret input or external payload reference.
    pub payload: Value,
    /// Optional idempotency key within the Run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dedupe_key: Option<String>,
    /// Queue lifecycle state.
    pub status: RunQueueItemStatus,
    /// Enqueue time.
    pub created_at: TimestampMs,
    /// Consumption/rejection time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub consumed_at: Option<TimestampMs>,
}

impl RunQueueItem {
    /// Validates identity, sequence, and terminal timestamp shape.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorCode::InvalidRun`] for an impossible queue item.
    pub fn validate(&self) -> Result<(), DomainError> {
        let finished = matches!(
            self.status,
            RunQueueItemStatus::Consumed | RunQueueItemStatus::Rejected
        );
        if self.id.as_str().is_empty()
            || self.run_id.as_str().is_empty()
            || self.sequence == 0
            || self.kind.as_str().is_empty()
            || finished != self.consumed_at.is_some()
            || self.consumed_at.is_some_and(|time| time < self.created_at)
        {
            return Err(DomainError::invariant(
                ErrorCode::InvalidRun,
                "run queue identity, sequence, state, or timestamps are inconsistent",
            ));
        }
        Ok(())
    }
}

/// Work class that prevents a Run from passing its termination barrier.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunTerminationBlocker {
    /// Agent invocation has not returned.
    AgentActive,
    /// A `ToolUse` or `ToolExecution` remains pending.
    ToolPending,
    /// A retry is running or scheduled.
    RetryPending,
    /// Compaction, checkpoint, or recovery remains active.
    RecoveryPending,
    /// An output or usage delta has not been persisted.
    PersistencePending,
    /// Run queue contains unconsumed work.
    QueueNotEmpty,
}

/// Snapshot used to decide whether a Run may atomically become completed.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RunTerminationReadiness {
    /// Work observed while evaluating the barrier.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub blockers: BTreeSet<RunTerminationBlocker>,
    /// Queue version observed when emptiness was read.
    pub observed_queue_version: u64,
    /// Queue version used by the completion compare-and-swap.
    pub current_queue_version: u64,
}

impl RunTerminationReadiness {
    /// Returns true only when every termination-barrier condition holds.
    #[must_use]
    pub fn can_complete(&self) -> bool {
        self.blockers.is_empty() && self.observed_queue_version == self.current_queue_version
    }
}

impl Run {
    /// Validates a worker mutation against the last acknowledged Run.
    /// # Errors
    /// Rejects changes to fixed input, counter rollback, or completion without barrier authority.
    pub fn validate_worker_successor(
        &self,
        after: &Self,
        completion: Option<bool>,
    ) -> Result<(), DomainError> {
        if self.status.is_terminal()
            || self.id != after.id
            || self.project_id != after.project_id
            || self.base_message_id != after.base_message_id
            || self.agent_id != after.agent_id
            || self.agent_revision != after.agent_revision
            || self.agent_snapshot != after.agent_snapshot
            || self.budget != after.budget
            || self.retry_policy != after.retry_policy
            || self.follow_session_id != after.follow_session_id
            || self.trigger != after.trigger
            || self.cron_id != after.cron_id
            || self.scheduled_at != after.scheduled_at
            || self.created_at != after.created_at
            || self.dedupe_key != after.dedupe_key
            || (self.started_at.is_some() && self.started_at != after.started_at)
            || (after.status == RunStatus::Queued && self.status != RunStatus::Queued)
            || after.step_count < self.step_count
            || after.step_count > self.step_count.saturating_add(1)
            || after.attempt_count < self.attempt_count
            || after.attempt_count > self.attempt_count.saturating_add(1)
            || after.compaction_count < self.compaction_count
            || after.compaction_count > self.compaction_count.saturating_add(1)
            || after.queue_version != self.queue_version
            || after.queue_cursor < self.queue_cursor
            || after.usage.input_tokens < self.usage.input_tokens
            || after.usage.cached_input_tokens < self.usage.cached_input_tokens
            || after.usage.output_tokens < self.usage.output_tokens
            || after.usage.tool_executions < self.usage.tool_executions
            || (self.usage.cost.is_some() && after.usage.cost < self.usage.cost)
            || (after.status == RunStatus::Completed
                && (completion != Some(true) || self.status != RunStatus::Settling))
        {
            return Err(DomainError::invariant(
                ErrorCode::InvalidRun,
                "Run successor changes fixed identity, counters or completion authority",
            ));
        }
        Ok(())
    }
    /// Stops an interrupted or cancelled execution without granting completion.
    /// # Errors
    /// Completion is reserved for the coordinator's termination barrier.
    pub fn stop(
        &mut self,
        reason: RunStopReason,
        at: TimestampMs,
        error: Option<DomainError>,
    ) -> Result<(), DomainError> {
        let status = match reason {
            RunStopReason::Completed => {
                return Err(DomainError::invariant(
                    ErrorCode::InvalidRun,
                    "completion requires the termination barrier",
                ));
            }
            RunStopReason::Cancelled => RunStatus::Cancelled,
            RunStopReason::Failed | RunStopReason::RetryExhausted => RunStatus::Failed,
            _ => RunStatus::LimitExceeded,
        };
        self.status = status;
        self.phase = RunPhase::Terminal;
        self.stop_reason = Some(reason);
        self.error = error;
        self.next_retry_at = None;
        self.ended_at.get_or_insert(at);
        Ok(())
    }
}

impl RunTrigger {
    /// Stable transport spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Manual => "manual",
            Self::Cron => "cron",
        }
    }
}

#[cfg(test)]
mod tests;
