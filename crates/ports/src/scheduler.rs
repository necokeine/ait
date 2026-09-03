use ait_domain::{
    AgentId, Cron, CronFire, CronFireState, CronId, DomainError, DurationMs, MessageId, ProjectId,
    RunId, SessionId, TimestampMs,
};
use async_trait::async_trait;

/// Atomic request to claim one occurrence and advance its Cron cursor.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClaimCronFire {
    /// Cron being advanced.
    pub cron_id: CronId,
    /// Project copied into the fire audit record.
    pub project_id: ProjectId,
    /// Configuration revision observed by the planner.
    pub expected_version: u64,
    /// Exact occurrence identity.
    pub scheduled_at: TimestampMs,
    /// First schedule instant after this claim's covered interval.
    pub next_run_at: Option<TimestampMs>,
    /// Claim wall-clock time.
    pub claimed_at: TimestampMs,
}

/// Result of the idempotent Cron claim transaction.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CronClaimResult {
    /// This caller inserted the fire and advanced the Cron cursor.
    Claimed(CronFire),
    /// The occurrence already exists; callers must not create a second Run.
    Existing(CronFire),
    /// The Cron configuration/cursor changed after it was read.
    Stale,
}

/// Trigger metadata accepted by the shared Run creation entry.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RunStartTrigger {
    /// Explicit interactive or background execution.
    Manual,
    /// One durable Cron occurrence.
    Cron {
        /// Source schedule.
        cron_id: CronId,
        /// Exact occurrence identity.
        scheduled_at: TimestampMs,
    },
}

/// Provider-independent request to create a queued Run.
///
/// Implementations validate the Message/Agent target, snapshot the current
/// enabled Agent revision, and persist the Run before returning. They never
/// invoke a provider directly.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StartRunRequest {
    /// Target Project.
    pub project_id: ProjectId,
    /// Immutable Message from which generation branches.
    pub base_message_id: MessageId,
    /// Agent resolved by the unified execution entry.
    pub agent_id: AgentId,
    /// Optional Session followed by interactive Runs; Cron always supplies `None`.
    pub follow_session_id: Option<SessionId>,
    /// Source metadata.
    pub trigger: RunStartTrigger,
    /// Stable request idempotency identity.
    pub dedupe_key: Option<String>,
    /// Optional wall-clock override.
    pub max_runtime: Option<DurationMs>,
}

/// Result of idempotent Run creation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RunStartResult {
    /// Created or previously existing Run.
    pub run_id: RunId,
    /// Whether this call inserted the Run.
    pub created: bool,
}

/// Non-terminal Run previously created by one Cron.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActiveCronRun {
    /// Active Run identity.
    pub run_id: RunId,
    /// Occurrence that created the Run.
    pub scheduled_at: TimestampMs,
}

/// Durable scheduler persistence boundary.
///
/// `claim_fire` atomically inserts `(cron_id, scheduled_at)` and advances
/// `last_run_at`/`next_run_at`; this is the restart-safe idempotency barrier.
#[async_trait]
pub trait CronStore: Send + Sync {
    /// Loads enabled schedules with `next_run_at <= now`, ordered by due time.
    async fn due_crons(&self, now: TimestampMs, limit: usize) -> Result<Vec<Cron>, DomainError>;

    /// Loads fires left claimed by an interrupted cross-database start saga.
    async fn claimed_fires(&self, limit: usize) -> Result<Vec<CronFire>, DomainError>;

    /// Loads current Cron configuration for recovery of a claimed fire.
    async fn load_cron(&self, id: &CronId) -> Result<Cron, DomainError>;

    /// Claims one occurrence and advances its Cron cursor atomically.
    async fn claim_fire(&self, request: ClaimCronFire) -> Result<CronClaimResult, DomainError>;

    /// Compares the current fire state and persists a lifecycle transition.
    async fn transition_fire(
        &self,
        fire: CronFire,
        expected: CronFireState,
    ) -> Result<CronFire, DomainError>;

    /// Lists non-terminal Runs created by this Cron only.
    async fn active_cron_runs(&self, cron_id: &CronId) -> Result<Vec<ActiveCronRun>, DomainError>;
}

/// Shared execution entry used by interactive and scheduled callers.
#[async_trait]
pub trait RunStarter: Send + Sync {
    /// Idempotently validates and persists a queued Run.
    async fn start_run(&self, request: StartRunRequest) -> Result<RunStartResult, DomainError>;

    /// Requests cooperative cancellation of one non-terminal Run.
    async fn cancel_run(&self, run_id: &RunId) -> Result<(), DomainError>;
}
