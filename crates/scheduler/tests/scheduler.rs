//! Contract tests for durable Cron planning and trigger orchestration.

use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

use ait_domain::{
    AgentId, Cron, CronConcurrencyPolicy, CronFire, CronFireState, CronId, CronMisfirePolicy,
    DomainError, DurationMs, ErrorCode, MessageId, ProjectId, RunId, TimestampMs,
};
use ait_ports::{
    ActiveCronRun, ClaimCronFire, CronClaimResult, CronStore, RunStartResult, RunStartTrigger,
    RunStarter, StartRunRequest,
};
use ait_scheduler::{CronScheduler, ScanMode};
use async_trait::async_trait;
use chrono::DateTime;

#[derive(Default)]
struct State {
    crons: BTreeMap<CronId, Cron>,
    fires: BTreeMap<(CronId, TimestampMs), CronFire>,
    active: BTreeMap<CronId, Vec<ActiveCronRun>>,
    requests: Vec<StartRunRequest>,
    runs_by_dedupe: BTreeMap<String, RunId>,
    cancelled: Vec<RunId>,
}

#[derive(Clone, Default)]
struct MemoryStore(Arc<Mutex<State>>);

impl MemoryStore {
    fn insert_cron(&self, cron: Cron) {
        self.0.lock().unwrap().crons.insert(cron.id.clone(), cron);
    }

    fn insert_claimed(&self, cron: &Cron, scheduled_at: TimestampMs, now: TimestampMs) {
        let fire = CronFire {
            cron_id: cron.id.clone(),
            scheduled_at,
            project_id: cron.project_id.clone(),
            state: CronFireState::Claimed,
            run_id: None,
            error: None,
            claimed_at: now,
            updated_at: now,
        };
        self.0
            .lock()
            .unwrap()
            .fires
            .insert((cron.id.clone(), scheduled_at), fire);
    }

    fn snapshot(&self) -> StateSnapshot {
        let state = self.0.lock().unwrap();
        StateSnapshot {
            crons: state.crons.clone(),
            fires: state.fires.clone(),
            requests: state.requests.clone(),
            cancelled: state.cancelled.clone(),
        }
    }
}

struct StateSnapshot {
    crons: BTreeMap<CronId, Cron>,
    fires: BTreeMap<(CronId, TimestampMs), CronFire>,
    requests: Vec<StartRunRequest>,
    cancelled: Vec<RunId>,
}

#[async_trait]
impl CronStore for MemoryStore {
    async fn due_crons(&self, now: TimestampMs, limit: usize) -> Result<Vec<Cron>, DomainError> {
        let state = self.0.lock().unwrap();
        let mut due: Vec<_> = state
            .crons
            .values()
            .filter(|cron| cron.enabled && cron.next_run_at.is_some_and(|next| next <= now))
            .cloned()
            .collect();
        due.sort_by_key(|cron| (cron.next_run_at, cron.id.clone()));
        due.truncate(limit);
        Ok(due)
    }

    async fn claimed_fires(&self, limit: usize) -> Result<Vec<CronFire>, DomainError> {
        let state = self.0.lock().unwrap();
        Ok(state
            .fires
            .values()
            .filter(|fire| fire.state == CronFireState::Claimed)
            .take(limit)
            .cloned()
            .collect())
    }

    async fn load_cron(&self, id: &CronId) -> Result<Cron, DomainError> {
        self.0
            .lock()
            .unwrap()
            .crons
            .get(id)
            .cloned()
            .ok_or_else(|| {
                DomainError::invariant(ErrorCode::InvalidCron, "cron is missing during recovery")
            })
    }

    async fn claim_fire(&self, request: ClaimCronFire) -> Result<CronClaimResult, DomainError> {
        let mut state = self.0.lock().unwrap();
        let key = (request.cron_id.clone(), request.scheduled_at);
        if let Some(fire) = state.fires.get(&key) {
            return Ok(CronClaimResult::Existing(fire.clone()));
        }
        let Some(cron) = state.crons.get_mut(&request.cron_id) else {
            return Ok(CronClaimResult::Stale);
        };
        if cron.version != request.expected_version
            || cron.project_id != request.project_id
            || cron.next_run_at != Some(request.scheduled_at)
        {
            return Ok(CronClaimResult::Stale);
        }
        cron.last_run_at = Some(request.scheduled_at);
        cron.next_run_at = request.next_run_at;
        cron.updated_at = request.claimed_at;
        let fire = CronFire {
            cron_id: request.cron_id,
            scheduled_at: request.scheduled_at,
            project_id: request.project_id,
            state: CronFireState::Claimed,
            run_id: None,
            error: None,
            claimed_at: request.claimed_at,
            updated_at: request.claimed_at,
        };
        fire.validate()?;
        state.fires.insert(key, fire.clone());
        Ok(CronClaimResult::Claimed(fire))
    }

    async fn transition_fire(
        &self,
        fire: CronFire,
        expected: CronFireState,
    ) -> Result<CronFire, DomainError> {
        fire.validate()?;
        let mut state = self.0.lock().unwrap();
        let key = (fire.cron_id.clone(), fire.scheduled_at);
        let current = state
            .fires
            .get_mut(&key)
            .ok_or_else(|| DomainError::invariant(ErrorCode::InvalidCron, "fire missing"))?;
        if current.state != expected {
            return Err(DomainError::transient(
                ErrorCode::CronDuplicateFire,
                "fire state compare-and-swap conflict",
            ));
        }
        *current = fire.clone();
        Ok(fire)
    }

    async fn active_cron_runs(&self, cron_id: &CronId) -> Result<Vec<ActiveCronRun>, DomainError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .active
            .get(cron_id)
            .cloned()
            .unwrap_or_default())
    }
}

#[derive(Clone)]
struct MemoryRuns(Arc<Mutex<State>>);

#[async_trait]
impl RunStarter for MemoryRuns {
    async fn start_run(&self, request: StartRunRequest) -> Result<RunStartResult, DomainError> {
        let mut state = self.0.lock().unwrap();
        state.requests.push(request.clone());
        let dedupe = request
            .dedupe_key
            .clone()
            .ok_or_else(|| DomainError::invariant(ErrorCode::InvalidRun, "dedupe missing"))?;
        if let Some(run_id) = state.runs_by_dedupe.get(&dedupe) {
            return Ok(RunStartResult {
                run_id: run_id.clone(),
                created: false,
            });
        }
        let run_id = RunId::new(format!("run-{}", state.runs_by_dedupe.len() + 1));
        state.runs_by_dedupe.insert(dedupe, run_id.clone());
        if let RunStartTrigger::Cron {
            cron_id,
            scheduled_at,
        } = request.trigger
        {
            state
                .active
                .entry(cron_id)
                .or_default()
                .push(ActiveCronRun {
                    run_id: run_id.clone(),
                    scheduled_at,
                });
        }
        Ok(RunStartResult {
            run_id,
            created: true,
        })
    }

    async fn cancel_run(&self, run_id: &RunId) -> Result<(), DomainError> {
        let mut state = self.0.lock().unwrap();
        state.cancelled.push(run_id.clone());
        for runs in state.active.values_mut() {
            runs.retain(|active| &active.run_id != run_id);
        }
        Ok(())
    }
}

fn timestamp(value: &str) -> TimestampMs {
    TimestampMs(
        DateTime::parse_from_rfc3339(value)
            .unwrap()
            .timestamp_millis(),
    )
}

fn cron(policy: CronMisfirePolicy, concurrency: CronConcurrencyPolicy) -> Cron {
    Cron {
        id: CronId::new("cron-1"),
        name: "minute job".into(),
        project_id: ProjectId::new("project-1"),
        base_message_id: MessageId::from_u128(1),
        agent_id: AgentId::new("agent-1"),
        schedule: "* * * * *".into(),
        timezone: "UTC".into(),
        enabled: true,
        concurrency_policy: concurrency,
        misfire_policy: policy,
        max_runtime: Some(DurationMs(30_000)),
        next_run_at: Some(timestamp("2026-09-04T00:01:00Z")),
        last_run_at: None,
        version: 1,
        created_at: timestamp("2026-09-03T00:00:00Z"),
        updated_at: timestamp("2026-09-03T00:00:00Z"),
    }
}

fn harness(cron: Cron) -> (CronScheduler, MemoryStore) {
    let store = MemoryStore::default();
    store.insert_cron(cron);
    let runs = MemoryRuns(store.0.clone());
    (
        CronScheduler::new(Arc::new(store.clone()), Arc::new(runs), 16),
        store,
    )
}

#[test]
fn schedule_uses_iana_timezone_and_enable_disable_resets_cursor() {
    let (scheduler, _) = harness(cron(
        CronMisfirePolicy::RunOnce,
        CronConcurrencyPolicy::Allow,
    ));
    let mut configured = cron(CronMisfirePolicy::RunOnce, CronConcurrencyPolicy::Allow);
    configured.schedule = "0 9 * * *".into();
    configured.timezone = "Asia/Shanghai".into();
    let now = timestamp("2026-09-04T00:30:00Z");
    let configured = scheduler.prepare_cron(configured, now).unwrap();
    assert_eq!(
        configured.next_run_at,
        Some(timestamp("2026-09-04T01:00:00Z"))
    );

    let disabled = scheduler.set_enabled(configured, false, now).unwrap();
    assert!(!disabled.enabled);
    assert_eq!(disabled.next_run_at, None);
    let enabled = scheduler.set_enabled(disabled, true, now).unwrap();
    assert!(enabled.enabled);
    assert_eq!(enabled.next_run_at, Some(timestamp("2026-09-04T01:00:00Z")));
    assert_eq!(enabled.version, 3);
}

#[test]
fn schedule_skips_a_nonexistent_dst_wall_clock_time() {
    let (scheduler, _) = harness(cron(
        CronMisfirePolicy::RunOnce,
        CronConcurrencyPolicy::Allow,
    ));
    let mut configured = cron(CronMisfirePolicy::RunOnce, CronConcurrencyPolicy::Allow);
    configured.schedule = "30 2 * * *".into();
    configured.timezone = "America/New_York".into();
    configured.created_at = timestamp("2026-03-01T00:00:00Z");
    configured.updated_at = configured.created_at;
    let configured = scheduler
        .prepare_cron(configured, timestamp("2026-03-08T06:00:00Z"))
        .unwrap();
    assert_eq!(
        configured.next_run_at,
        Some(timestamp("2026-03-09T06:30:00Z"))
    );
}

#[test]
fn invalid_timezone_and_expression_are_rejected() {
    let (scheduler, _) = harness(cron(
        CronMisfirePolicy::RunOnce,
        CronConcurrencyPolicy::Allow,
    ));
    let mut invalid = cron(CronMisfirePolicy::RunOnce, CronConcurrencyPolicy::Allow);
    invalid.timezone = "Mars/Olympus".into();
    assert_eq!(
        scheduler
            .prepare_cron(invalid, timestamp("2026-09-04T00:00:00Z"))
            .unwrap_err()
            .code,
        ErrorCode::InvalidCron
    );

    let mut invalid = cron(CronMisfirePolicy::RunOnce, CronConcurrencyPolicy::Allow);
    invalid.schedule = "not a schedule".into();
    assert_eq!(
        scheduler
            .prepare_cron(invalid, timestamp("2026-09-04T00:00:00Z"))
            .unwrap_err()
            .code,
        ErrorCode::InvalidCron
    );
}

#[tokio::test]
async fn recovery_applies_skip_run_once_and_catch_up() {
    let now = timestamp("2026-09-04T00:03:30Z");
    let cases = [
        (CronMisfirePolicy::Skip, 1, 0, CronFireState::Skipped),
        (CronMisfirePolicy::RunOnce, 1, 1, CronFireState::Started),
        (CronMisfirePolicy::CatchUp, 3, 3, CronFireState::Started),
    ];

    for (policy, fire_count, request_count, final_state) in cases {
        let (scheduler, store) = harness(cron(policy, CronConcurrencyPolicy::Allow));
        let report = scheduler.scan(now, ScanMode::Recovery, 10).await.unwrap();
        assert_eq!(report.fires.len(), fire_count);
        assert!(report.fires.iter().all(|fire| fire.state == final_state));
        let snapshot = store.snapshot();
        assert_eq!(snapshot.requests.len(), request_count);
        assert_eq!(
            snapshot.crons[&CronId::new("cron-1")].next_run_at,
            Some(timestamp("2026-09-04T00:04:00Z"))
        );
    }
}

#[tokio::test]
async fn concurrency_policies_allow_block_or_replace_same_cron_runs() {
    let now = timestamp("2026-09-04T00:01:00Z");
    for (policy, expected_state, requests, cancellations) in [
        (CronConcurrencyPolicy::Allow, CronFireState::Started, 1, 0),
        (CronConcurrencyPolicy::Forbid, CronFireState::Blocked, 0, 0),
        (CronConcurrencyPolicy::Replace, CronFireState::Started, 1, 1),
    ] {
        let configured = cron(CronMisfirePolicy::RunOnce, policy);
        let (scheduler, store) = harness(configured.clone());
        store.0.lock().unwrap().active.insert(
            configured.id.clone(),
            vec![ActiveCronRun {
                run_id: RunId::new("old-run"),
                scheduled_at: timestamp("2026-09-03T23:59:00Z"),
            }],
        );

        let report = scheduler.scan(now, ScanMode::Regular, 10).await.unwrap();
        assert_eq!(report.fires[0].state, expected_state);
        let snapshot = store.snapshot();
        assert_eq!(snapshot.requests.len(), requests);
        assert_eq!(snapshot.cancelled.len(), cancellations);
    }
}

#[tokio::test]
async fn claimed_saga_recovers_existing_run_without_duplicate() {
    let scheduled_at = timestamp("2026-09-04T00:01:00Z");
    let now = timestamp("2026-09-04T00:02:00Z");
    let mut configured = cron(CronMisfirePolicy::RunOnce, CronConcurrencyPolicy::Forbid);
    configured.next_run_at = Some(timestamp("2026-09-04T00:03:00Z"));
    configured.last_run_at = Some(scheduled_at);
    let (scheduler, store) = harness(configured.clone());
    store.insert_claimed(&configured, scheduled_at, scheduled_at);
    let dedupe = configured.fire_dedupe_key(scheduled_at);
    let existing_run = RunId::new("existing-run");
    {
        let mut state = store.0.lock().unwrap();
        state.runs_by_dedupe.insert(dedupe, existing_run.clone());
        state.active.insert(
            configured.id.clone(),
            vec![ActiveCronRun {
                run_id: existing_run.clone(),
                scheduled_at,
            }],
        );
    }

    let report = scheduler.scan(now, ScanMode::Recovery, 10).await.unwrap();
    assert_eq!(report.fires.len(), 1);
    assert_eq!(report.fires[0].state, CronFireState::Started);
    assert_eq!(report.fires[0].run_id, Some(existing_run));
    let request = &store.snapshot().requests[0];
    assert_eq!(request.project_id, configured.project_id);
    assert_eq!(request.base_message_id, configured.base_message_id);
    assert_eq!(request.agent_id, configured.agent_id);
    assert_eq!(request.follow_session_id, None);
    assert_eq!(
        request.dedupe_key,
        Some(configured.fire_dedupe_key(scheduled_at))
    );

    let second = scheduler.scan(now, ScanMode::Recovery, 10).await.unwrap();
    assert!(second.fires.is_empty());
    let snapshot = store.snapshot();
    assert_eq!(snapshot.requests.len(), 1);
    assert_eq!(snapshot.fires.len(), 1);
}
