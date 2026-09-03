use serde::{Deserialize, Serialize};

use crate::{
    AgentId, DomainError, DurationMs, ErrorCode, MessageId, ProjectId, RunId, TimestampMs,
};

/// Stable identity of a Cron schedule.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CronId(String);

impl CronId {
    /// Creates an externally assigned Cron identity.
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

/// Handling of overlapping Runs created by the same Cron.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CronConcurrencyPolicy {
    /// Start every due occurrence.
    Allow,
    /// Skip a due occurrence while another Run is non-terminal.
    Forbid,
    /// Cancel the existing Run before starting the due occurrence.
    Replace,
}

/// Handling of occurrences missed while the scheduler was unavailable.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CronMisfirePolicy {
    /// Ignore all missed occurrences.
    Skip,
    /// Start one Run representing the missed interval.
    RunOnce,
    /// Replay each missed occurrence in schedule order.
    CatchUp,
}

/// Durable lifecycle of one scheduled Cron occurrence.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CronFireState {
    /// The global scheduler cursor and occurrence identity were claimed.
    Claimed,
    /// The unified Run entry accepted the occurrence.
    Started,
    /// Recovery policy intentionally ignored the occurrence.
    Skipped,
    /// Concurrency policy prevented the occurrence from starting.
    Blocked,
    /// Target validation or Run creation failed permanently for this occurrence.
    Failed,
}

/// Auditable bridge from a scheduled occurrence to its Run.
///
/// `(cron_id, scheduled_at)` is the durable idempotency identity. A started
/// fire links to a Run, whose `base_message_id` and `last_message_id` expose
/// the input branch point and eventual result branch respectively.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CronFire {
    /// Source Cron.
    pub cron_id: CronId,
    /// Exact scheduled instant represented by this occurrence.
    pub scheduled_at: TimestampMs,
    /// Project copied from the Cron for cross-database routing and validation.
    pub project_id: ProjectId,
    /// Fire lifecycle.
    pub state: CronFireState,
    /// Created or recovered Run, present only after a successful start.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<RunId>,
    /// Stable safe failure, present only for a failed occurrence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<DomainError>,
    /// Time at which the scheduler first claimed this occurrence.
    pub claimed_at: TimestampMs,
    /// Last fire-state update.
    pub updated_at: TimestampMs,
}

impl CronFire {
    /// Validates the persisted fire envelope.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorCode::InvalidCron`] when identity, state payload, or
    /// timestamps are inconsistent.
    pub fn validate(&self) -> Result<(), DomainError> {
        let payload_valid = match self.state {
            CronFireState::Started => self.run_id.is_some() && self.error.is_none(),
            CronFireState::Failed => self.run_id.is_none() && self.error.is_some(),
            CronFireState::Claimed | CronFireState::Skipped | CronFireState::Blocked => {
                self.run_id.is_none() && self.error.is_none()
            }
        };
        if self.cron_id.as_str().is_empty()
            || self.project_id.as_str().is_empty()
            || !payload_valid
            || self.updated_at < self.claimed_at
        {
            return Err(DomainError::invariant(
                ErrorCode::InvalidCron,
                "cron fire identity, state payload, or timestamps are invalid",
            ));
        }
        Ok(())
    }
}

/// Recurring trigger with a fixed Project, base Message, and Agent target.
///
/// A fire resolves the Agent's then-current enabled revision into the new Run;
/// the Cron never creates or moves a Session.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Cron {
    /// Cron identity.
    pub id: CronId,
    /// Human-readable name.
    pub name: String,
    /// Fixed Project boundary.
    pub project_id: ProjectId,
    /// Fixed Message from which each occurrence branches.
    pub base_message_id: MessageId,
    /// Fixed Agent whose current revision is snapshotted at fire time.
    pub agent_id: AgentId,
    /// Cron expression interpreted in `timezone`.
    pub schedule: String,
    /// IANA timezone name.
    pub timezone: String,
    /// Whether new occurrences may fire.
    pub enabled: bool,
    /// Overlap handling for Runs from this Cron only.
    pub concurrency_policy: CronConcurrencyPolicy,
    /// Missed-occurrence handling.
    pub misfire_policy: CronMisfirePolicy,
    /// Optional Run wall-clock limit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_runtime: Option<DurationMs>,
    /// Next due occurrence calculated by the scheduler.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_run_at: Option<TimestampMs>,
    /// Most recently handled occurrence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_run_at: Option<TimestampMs>,
    /// Compare-and-swap version for configuration updates.
    pub version: u64,
    /// Creation time.
    pub created_at: TimestampMs,
    /// Last configuration update time.
    pub updated_at: TimestampMs,
}

impl Cron {
    /// Validates the fixed target and scheduling fields.
    ///
    /// Cron syntax and timezone existence are adapter concerns; the domain
    /// requires both canonical strings to be non-empty.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorCode::InvalidCron`] for an invalid aggregate.
    pub fn validate(&self) -> Result<(), DomainError> {
        if self.id.as_str().is_empty()
            || self.name.trim().is_empty()
            || self.project_id.as_str().is_empty()
            || self.base_message_id.as_uuid().is_nil()
            || self.agent_id.as_str().is_empty()
            || self.schedule.trim().is_empty()
            || self.timezone.trim().is_empty()
            || self.version == 0
            || self.updated_at < self.created_at
            || self
                .last_run_at
                .zip(self.next_run_at)
                .is_some_and(|(last, next)| next <= last)
        {
            return Err(DomainError::invariant(
                ErrorCode::InvalidCron,
                "cron identity, target, schedule, version, or timestamps are invalid",
            ));
        }
        Ok(())
    }

    /// Returns the deterministic fire idempotency key.
    #[must_use]
    pub fn fire_dedupe_key(&self, scheduled_at: TimestampMs) -> String {
        format!("{}:{}", self.id.as_str(), scheduled_at.get())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cron() -> Cron {
        Cron {
            id: CronId::new("cron-1"),
            name: "nightly".into(),
            project_id: ProjectId::new("project-1"),
            base_message_id: MessageId::from_u128(1),
            agent_id: AgentId::new("agent-1"),
            schedule: "0 0 * * *".into(),
            timezone: "Asia/Shanghai".into(),
            enabled: true,
            concurrency_policy: CronConcurrencyPolicy::Forbid,
            misfire_policy: CronMisfirePolicy::RunOnce,
            max_runtime: Some(DurationMs(60_000)),
            next_run_at: Some(TimestampMs(200)),
            last_run_at: Some(TimestampMs(100)),
            version: 1,
            created_at: TimestampMs(1),
            updated_at: TimestampMs(2),
        }
    }

    #[test]
    fn target_is_valid_and_dedupe_key_is_stable() {
        let mut cron = cron();
        cron.validate().unwrap();
        assert_eq!(cron.fire_dedupe_key(TimestampMs(123)), "cron-1:123");
        cron.schedule.clear();
        assert_eq!(cron.validate().unwrap_err().code, ErrorCode::InvalidCron);
    }

    #[test]
    fn policies_round_trip_as_snake_case() {
        let encoded = serde_json::to_string(&cron()).unwrap();
        assert!(encoded.contains("\"concurrency_policy\":\"forbid\""));
        assert!(encoded.contains("\"misfire_policy\":\"run_once\""));
        assert_eq!(serde_json::from_str::<Cron>(&encoded).unwrap(), cron());
    }

    #[test]
    fn fire_requires_a_run_only_after_start() {
        let mut fire = CronFire {
            cron_id: CronId::new("cron-1"),
            scheduled_at: TimestampMs(100),
            project_id: ProjectId::new("project-1"),
            state: CronFireState::Claimed,
            run_id: None,
            error: None,
            claimed_at: TimestampMs(101),
            updated_at: TimestampMs(101),
        };
        fire.validate().unwrap();

        fire.state = CronFireState::Started;
        assert_eq!(fire.validate().unwrap_err().code, ErrorCode::InvalidCron);
        fire.run_id = Some(RunId::new("run-1"));
        fire.validate().unwrap();
    }
}
