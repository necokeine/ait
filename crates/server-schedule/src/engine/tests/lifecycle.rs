//! Paseo schedule/service.test.ts cadence, manual run and terminal-state cases.
use super::*;

#[test]
fn interval_opt_out_waits_the_full_interval_before_first_run() {
    let (mut engine, _) = fixture();
    let mut params = input();
    params["runOnCreate"] = json!(false);
    let id = engine
        .request("schedule.create.request", params, now())
        .unwrap()["schedule"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(
        engine
            .due(now() + chrono::Duration::milliseconds(59_999))
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        engine.due(now() + chrono::Duration::minutes(1)).unwrap(),
        vec![id]
    );
}

#[test]
fn manual_active_run_preserves_next_slot_even_after_count_limit() {
    let (mut engine, _) = fixture();
    let id = create(&mut engine);
    engine
        .request(
            "schedule.update.request",
            json!({"scheduleId":id,"maxRuns":1}),
            now(),
        )
        .unwrap();
    let original_next = engine.inspect(&id).unwrap().next_run_at;
    let later = now() + chrono::Duration::seconds(10);
    let (_, run) = engine.begin(&id, true, later).unwrap();
    engine
        .finish(&id, &run, true, Outcome::default(), later)
        .unwrap();
    let record = engine.inspect(&id).unwrap();
    assert_eq!(record.status, Status::Active);
    assert_eq!(record.next_run_at, original_next);
    assert_eq!(record.runs[0].scheduled_for, later);
    assert_eq!(record.last_run_at, Some(later));
    assert!(engine.due(later).unwrap().is_empty());
    assert_eq!(engine.inspect(&id).unwrap().status, Status::Completed);
}

#[test]
fn transient_failure_keeps_schedule_active_and_skips_missed_slots() {
    let (mut engine, _) = fixture();
    let id = create(&mut engine);
    let (_, run) = engine.begin(&id, false, now()).unwrap();
    let end = now() + chrono::Duration::minutes(3) + chrono::Duration::seconds(12);
    engine
        .finish(
            &id,
            &run,
            false,
            Outcome {
                error: Some("temporary provider failure".into()),
                ..Outcome::default()
            },
            end,
        )
        .unwrap();
    let record = engine.inspect(&id).unwrap();
    assert_eq!(record.status, Status::Active);
    assert_eq!(record.runs[0].status, RunStatus::Failed);
    assert_eq!(
        record.next_run_at,
        Some(now() + chrono::Duration::minutes(4))
    );
    assert!(engine.due(end).unwrap().is_empty());
}

#[test]
fn target_gone_completes_only_the_affected_schedule() {
    let (mut engine, _) = fixture();
    let affected = create(&mut engine);
    let sibling = create(&mut engine);
    let (_, run) = engine.begin(&affected, false, now()).unwrap();
    engine
        .finish(
            &affected,
            &run,
            false,
            Outcome {
                target_gone: true,
                error: Some("target is gone".into()),
                ..Outcome::default()
            },
            now(),
        )
        .unwrap();
    assert_eq!(engine.inspect(&affected).unwrap().status, Status::Completed);
    assert_eq!(engine.inspect(&affected).unwrap().next_run_at, None);
    assert_eq!(engine.inspect(&sibling).unwrap().status, Status::Active);
    assert_eq!(engine.due(now()).unwrap(), vec![sibling]);
}

#[test]
fn automatic_ticks_ignore_inflight_schedule_while_other_schedules_remain_due() {
    let (mut engine, _) = fixture();
    let running = create(&mut engine);
    let waiting = create(&mut engine);
    engine.begin(&running, false, now()).unwrap();
    assert_eq!(
        engine.due(now() + chrono::Duration::minutes(2)).unwrap(),
        vec![waiting]
    );
    assert_eq!(
        engine.begin(&running, false, now()).unwrap_err(),
        Error::Conflict
    );
    assert_eq!(
        engine.begin(&running, true, now()).unwrap_err(),
        Error::Conflict
    );
}

#[test]
fn completing_after_pause_clears_pause_metadata_when_expired() {
    let (mut engine, _) = fixture();
    let id = create(&mut engine);
    let (_, run) = engine.begin(&id, false, now()).unwrap();
    engine
        .request("schedule.pause.request", json!({"scheduleId":id}), now())
        .unwrap();
    engine
        .request(
            "schedule.update.request",
            json!({"scheduleId":id,"expiresAt":now()}),
            now(),
        )
        .unwrap();
    engine
        .finish(&id, &run, false, Outcome::default(), now())
        .unwrap();
    let record = engine.inspect(&id).unwrap();
    assert_eq!(record.status, Status::Completed);
    assert_eq!(record.next_run_at, None);
    assert_eq!(record.paused_at, None);
    assert_eq!(
        engine
            .request("schedule.pause.request", json!({"scheduleId":id}), now())
            .unwrap_err(),
        Error::Conflict
    );
}

#[test]
fn checkpoint_identities_survive_outcomes_that_omit_them() {
    let (mut engine, _) = fixture();
    let id = create(&mut engine);
    let (_, run) = engine.begin(&id, true, now()).unwrap();
    let (reply, _) = tokio::sync::oneshot::channel();
    engine
        .checkpoint(&crate::ports::Checkpoint {
            schedule_id: id.clone(),
            run_id: run.clone(),
            agent_id: Some("allocated-agent".into()),
            workspace_id: Some("allocated-workspace".into()),
            reply,
        })
        .unwrap();
    engine
        .finish(
            &id,
            &run,
            true,
            Outcome {
                error: Some("execution failed".into()),
                ..Outcome::default()
            },
            now(),
        )
        .unwrap();
    let record = engine.inspect(&id).unwrap();
    assert_eq!(record.runs[0].agent_id.as_deref(), Some("allocated-agent"));
    assert_eq!(
        record.runs[0].workspace_id.as_deref(),
        Some("allocated-workspace")
    );
    let (reply, _) = tokio::sync::oneshot::channel();
    assert_eq!(
        engine
            .checkpoint(&crate::ports::Checkpoint {
                schedule_id: id,
                run_id: run,
                agent_id: None,
                workspace_id: None,
                reply
            })
            .unwrap_err(),
        Error::NotFound
    );
}

#[test]
fn output_and_error_limits_preserve_utf8_and_completed_history() {
    let (mut engine, store) = fixture();
    let id = create(&mut engine);
    let (_, run) = engine.begin(&id, true, now()).unwrap();
    let message = "界".repeat(30_000);
    engine
        .finish(
            &id,
            &run,
            true,
            Outcome {
                output: Some(message.clone()),
                error: Some(message),
                ..Outcome::default()
            },
            now(),
        )
        .unwrap();
    let record = engine.inspect(&id).unwrap();
    for text in [&record.runs[0].output, &record.runs[0].error] {
        let text = text.as_deref().unwrap();
        assert_eq!(text.len(), 65_535);
        assert_eq!(text.chars().count(), 21_845);
    }
    assert_eq!(json!(store.0.lock().unwrap().0[0].runs), json!(record.runs));
}
