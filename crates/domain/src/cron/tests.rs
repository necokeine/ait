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
    assert_eq!(
        cron_session_id(&cron.id, TimestampMs(123)),
        cron_session_id(&cron.id, TimestampMs(123))
    );
    assert_ne!(
        cron_session_id(&cron.id, TimestampMs(123)),
        cron_session_id(&cron.id, TimestampMs(124))
    );
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
