//! Durable Cron planning, restart recovery, and Run trigger orchestration.
//!
//! This crate owns time calculation and policy decisions. Persistence remains
//! behind [`ait_ports::CronStore`], while all execution is routed through
//! [`ait_ports::RunStarter`]; the scheduler never invokes a provider.

use std::{str::FromStr, sync::Arc};

use ait_domain::{
    Cron, CronConcurrencyPolicy, CronFire, CronFireState, CronId, CronMisfirePolicy, DomainError,
    ErrorCode, TimestampMs,
};
use ait_ports::{
    ClaimCronFire, CronClaimResult, CronStore, RunStartTrigger, RunStarter, StartRunRequest,
};
use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use cron::Schedule;

/// Whether a scan is part of normal polling or daemon-start recovery.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScanMode {
    /// Handle the next due occurrence for each schedule.
    Regular,
    /// Apply each Cron's configured misfire policy to downtime occurrences.
    Recovery,
}

/// Observable result of one bounded scheduler scan.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ScanReport {
    /// Fires handled or recovered by this scan, in processing order.
    pub fires: Vec<CronFire>,
    /// Crons whose persisted configuration changed after planning.
    pub stale_crons: Vec<CronId>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PlannedAction {
    Start,
    Skip,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PlannedOccurrence {
    scheduled_at: TimestampMs,
    next_run_at: Option<TimestampMs>,
    action: PlannedAction,
}

struct ParsedSchedule {
    schedule: Schedule,
    timezone: Tz,
}

impl ParsedSchedule {
    fn parse(expression: &str, timezone: &str) -> Result<Self, DomainError> {
        let expression = expression.trim();
        let normalized = if expression.split_whitespace().count() == 5 {
            format!("0 {expression}")
        } else {
            expression.to_owned()
        };
        let schedule = Schedule::from_str(&normalized).map_err(|error| {
            invalid_cron(format!("invalid cron schedule `{expression}`: {error}"))
        })?;
        let timezone = Tz::from_str(timezone.trim()).map_err(|error| {
            invalid_cron(format!("invalid IANA timezone `{timezone}`: {error}"))
        })?;
        Ok(Self { schedule, timezone })
    }

    fn next_after(&self, timestamp: TimestampMs) -> Result<Option<TimestampMs>, DomainError> {
        let instant = DateTime::<Utc>::from_timestamp_millis(timestamp.get()).ok_or_else(|| {
            invalid_cron(format!(
                "timestamp is outside the supported date-time range: {}",
                timestamp.get()
            ))
        })?;
        let local = instant.with_timezone(&self.timezone);
        Ok(self
            .schedule
            .after(&local)
            .next()
            .map(|next| TimestampMs(next.timestamp_millis())))
    }
}

/// Validates a Cron expression and IANA timezone, returning the next occurrence.
///
/// Five-field expressions are normalized by adding a zero-seconds field, matching
/// [`CronScheduler::prepare_cron`].
///
/// # Errors
///
/// Returns [`ErrorCode::InvalidCron`] for invalid syntax, timezone, or timestamp.
pub fn next_occurrence(
    expression: &str,
    timezone: &str,
    after: TimestampMs,
) -> Result<Option<TimestampMs>, DomainError> {
    ParsedSchedule::parse(expression, timezone)?.next_after(after)
}

/// Durable scheduler service composed from persistence and unified Run ports.
pub struct CronScheduler {
    store: Arc<dyn CronStore>,
    runs: Arc<dyn RunStarter>,
    max_catch_up_per_cron: usize,
}

impl CronScheduler {
    /// Creates a scheduler with a positive per-Cron catch-up bound.
    ///
    /// A zero bound is normalized to one so an enabled `catch_up` schedule can
    /// always make progress.
    #[must_use]
    pub fn new(
        store: Arc<dyn CronStore>,
        runs: Arc<dyn RunStarter>,
        max_catch_up_per_cron: usize,
    ) -> Self {
        Self {
            store,
            runs,
            max_catch_up_per_cron: max_catch_up_per_cron.max(1),
        }
    }

    /// Validates parser-level configuration and initializes the schedule cursor.
    ///
    /// Disabled schedules retain `last_run_at` but have no `next_run_at`.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorCode::InvalidCron`] for an invalid expression, timezone,
    /// domain aggregate, or unsupported timestamp.
    pub fn prepare_cron(&self, mut cron: Cron, now: TimestampMs) -> Result<Cron, DomainError> {
        let parsed = ParsedSchedule::parse(&cron.schedule, &cron.timezone)?;
        cron.next_run_at = if cron.enabled {
            parsed.next_after(now)?
        } else {
            None
        };
        cron.updated_at = now;
        cron.validate()?;
        Ok(cron)
    }

    /// Enables or disables a Cron and advances its configuration version.
    ///
    /// Enabling begins strictly after `now`; it never retroactively invents a
    /// misfire interval for time spent disabled.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorCode::InvalidCron`] when parser/domain validation fails
    /// or the version counter is exhausted.
    pub fn set_enabled(
        &self,
        mut cron: Cron,
        enabled: bool,
        now: TimestampMs,
    ) -> Result<Cron, DomainError> {
        cron.version = cron
            .version
            .checked_add(1)
            .ok_or_else(|| invalid_cron("cron version counter overflow"))?;
        cron.enabled = enabled;
        self.prepare_cron(cron, now)
    }

    /// Recovers interrupted fire sagas, claims due occurrences, and routes each
    /// runnable fire through the shared Run start entry.
    ///
    /// `limit` bounds both recovered and newly claimed fire outcomes. A zero
    /// limit is a side-effect-free no-op.
    ///
    /// # Errors
    ///
    /// Returns a store or retryable execution error. Retryable failures leave a
    /// fire in `claimed`, so the next scan resumes the same idempotency key.
    pub async fn scan(
        &self,
        now: TimestampMs,
        mode: ScanMode,
        limit: usize,
    ) -> Result<ScanReport, DomainError> {
        if limit == 0 {
            return Ok(ScanReport::default());
        }

        let mut report = ScanReport::default();
        let claimed = self.store.claimed_fires(limit).await?;
        for fire in claimed {
            if report.fires.len() == limit {
                return Ok(report);
            }
            let cron = self.store.load_cron(&fire.cron_id).await?;
            let fire = self.process_claimed(&cron, fire, now).await?;
            report.fires.push(fire);
        }

        let remaining = limit.saturating_sub(report.fires.len());
        if remaining == 0 {
            return Ok(report);
        }
        let due_crons = self.store.due_crons(now, remaining).await?;
        for cron in due_crons {
            if report.fires.len() == limit {
                break;
            }
            let available = limit - report.fires.len();
            let plans = self.plan(&cron, now, mode, available)?;
            for plan in plans {
                let claim = self
                    .store
                    .claim_fire(ClaimCronFire {
                        cron_id: cron.id.clone(),
                        project_id: cron.project_id.clone(),
                        expected_version: cron.version,
                        scheduled_at: plan.scheduled_at,
                        next_run_at: plan.next_run_at,
                        claimed_at: now,
                    })
                    .await?;

                match claim {
                    CronClaimResult::Claimed(fire) => {
                        let fire = match plan.action {
                            PlannedAction::Start => self.process_claimed(&cron, fire, now).await?,
                            PlannedAction::Skip => {
                                self.transition(fire, CronFireState::Skipped, None, now)
                                    .await?
                            }
                        };
                        report.fires.push(fire);
                    }
                    CronClaimResult::Existing(fire) => {
                        if fire.state == CronFireState::Claimed {
                            report
                                .fires
                                .push(self.process_claimed(&cron, fire, now).await?);
                        } else {
                            report.fires.push(fire);
                        }
                    }
                    CronClaimResult::Stale => {
                        report.stale_crons.push(cron.id.clone());
                        break;
                    }
                }
            }
        }
        Ok(report)
    }

    fn plan(
        &self,
        cron: &Cron,
        now: TimestampMs,
        mode: ScanMode,
        available: usize,
    ) -> Result<Vec<PlannedOccurrence>, DomainError> {
        cron.validate()?;
        if !cron.enabled || available == 0 {
            return Ok(Vec::new());
        }
        let Some(first) = cron.next_run_at.filter(|due| *due <= now) else {
            return Ok(Vec::new());
        };
        let parsed = ParsedSchedule::parse(&cron.schedule, &cron.timezone)?;

        if mode == ScanMode::Recovery {
            match cron.misfire_policy {
                CronMisfirePolicy::Skip => {
                    return Ok(vec![PlannedOccurrence {
                        scheduled_at: first,
                        next_run_at: parsed.next_after(now)?,
                        action: PlannedAction::Skip,
                    }]);
                }
                CronMisfirePolicy::RunOnce => {
                    return Ok(vec![PlannedOccurrence {
                        scheduled_at: first,
                        next_run_at: parsed.next_after(now)?,
                        action: PlannedAction::Start,
                    }]);
                }
                CronMisfirePolicy::CatchUp => {}
            }
        }

        let take = if mode == ScanMode::Recovery {
            available.min(self.max_catch_up_per_cron)
        } else {
            1
        };
        let mut plans = Vec::with_capacity(take);
        let mut scheduled_at = first;
        while plans.len() < take && scheduled_at <= now {
            let next_run_at = parsed.next_after(scheduled_at)?;
            plans.push(PlannedOccurrence {
                scheduled_at,
                next_run_at,
                action: PlannedAction::Start,
            });
            let Some(next) = next_run_at else { break };
            scheduled_at = next;
        }
        Ok(plans)
    }

    async fn process_claimed(
        &self,
        cron: &Cron,
        fire: CronFire,
        now: TimestampMs,
    ) -> Result<CronFire, DomainError> {
        fire.validate()?;
        if fire.state != CronFireState::Claimed {
            return Ok(fire);
        }
        if fire.cron_id != cron.id || fire.project_id != cron.project_id {
            return self
                .record_failure(
                    fire,
                    DomainError::invariant(
                        ErrorCode::InvalidCron,
                        "claimed fire does not match its Cron target",
                    ),
                    now,
                )
                .await;
        }

        let active = self.store.active_cron_runs(&cron.id).await?;
        let same_occurrence_exists = active
            .iter()
            .any(|run| run.scheduled_at == fire.scheduled_at);
        if !same_occurrence_exists {
            match cron.concurrency_policy {
                CronConcurrencyPolicy::Forbid if !active.is_empty() => {
                    return self
                        .transition(fire, CronFireState::Blocked, None, now)
                        .await;
                }
                CronConcurrencyPolicy::Allow | CronConcurrencyPolicy::Forbid => {}
                CronConcurrencyPolicy::Replace => {
                    for active_run in active {
                        if let Err(error) = self.runs.cancel_run(&active_run.run_id).await {
                            if error.retryable {
                                return Err(error);
                            }
                            return self.record_failure(fire, error, now).await;
                        }
                    }
                }
            }
        }

        let request = StartRunRequest {
            project_id: cron.project_id.clone(),
            base_message_id: cron.base_message_id,
            agent_id: cron.agent_id.clone(),
            follow_session_id: None,
            trigger: RunStartTrigger::Cron {
                cron_id: cron.id.clone(),
                scheduled_at: fire.scheduled_at,
            },
            dedupe_key: Some(cron.fire_dedupe_key(fire.scheduled_at)),
            max_runtime: cron.max_runtime,
        };
        match self.runs.start_run(request).await {
            Ok(started) => {
                self.transition(fire, CronFireState::Started, Some(started.run_id), now)
                    .await
            }
            Err(error) if error.retryable => Err(error),
            Err(error) => self.record_failure(fire, error, now).await,
        }
    }

    async fn record_failure(
        &self,
        mut fire: CronFire,
        error: DomainError,
        now: TimestampMs,
    ) -> Result<CronFire, DomainError> {
        fire.state = CronFireState::Failed;
        fire.error = Some(error);
        fire.updated_at = now;
        fire.validate()?;
        self.store
            .transition_fire(fire, CronFireState::Claimed)
            .await
    }

    async fn transition(
        &self,
        mut fire: CronFire,
        state: CronFireState,
        run_id: Option<ait_domain::RunId>,
        now: TimestampMs,
    ) -> Result<CronFire, DomainError> {
        fire.state = state;
        fire.run_id = run_id;
        fire.error = None;
        fire.updated_at = now;
        fire.validate()?;
        self.store
            .transition_fire(fire, CronFireState::Claimed)
            .await
    }
}

fn invalid_cron(message: impl Into<String>) -> DomainError {
    DomainError::invariant(ErrorCode::InvalidCron, message)
}
