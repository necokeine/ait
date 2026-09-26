//! Durable failure and restart extensions of Paseo schedule/service.test.ts.
use super::*;

#[test]
fn failed_completion_write_is_retryable_without_a_second_occurrence() {
    let (mut engine, store) = fixture();
    let id = create(&mut engine);
    let (_, run) = engine.begin(&id, false, now()).unwrap();
    let outcome = Outcome {
        output: Some("only executed once".into()),
        ..Outcome::default()
    };
    store.0.lock().unwrap().1 = true;
    assert_eq!(
        engine
            .finish(&id, &run, false, outcome.clone(), now())
            .unwrap_err(),
        Error::Storage
    );
    assert_eq!(
        engine.inspect(&id).unwrap().runs[0].status,
        RunStatus::Running
    );
    assert_eq!(
        store.0.lock().unwrap().0[0].runs[0].status,
        RunStatus::Running
    );
    assert!(engine.due(now()).unwrap().is_empty());
    store.0.lock().unwrap().1 = false;
    engine.finish(&id, &run, false, outcome, now()).unwrap();
    let record = engine.inspect(&id).unwrap();
    assert_eq!(record.runs.len(), 1);
    assert_eq!(record.runs[0].status, RunStatus::Succeeded);
    assert_eq!(record.runs[0].output.as_deref(), Some("only executed once"));
}

#[test]
fn failed_checkpoint_write_does_not_publish_uncommitted_identities() {
    let (mut engine, store) = fixture();
    let id = create(&mut engine);
    let (_, run) = engine.begin(&id, true, now()).unwrap();
    let (reply, _) = tokio::sync::oneshot::channel();
    let checkpoint = crate::ports::Checkpoint {
        schedule_id: id.clone(),
        run_id: run,
        agent_id: Some("agent".into()),
        workspace_id: Some("workspace".into()),
        reply,
    };
    store.0.lock().unwrap().1 = true;
    assert_eq!(engine.checkpoint(&checkpoint).unwrap_err(), Error::Storage);
    assert_eq!(engine.inspect(&id).unwrap().runs[0].agent_id, None);
    store.0.lock().unwrap().1 = false;
    engine.checkpoint(&checkpoint).unwrap();
    assert_eq!(
        engine.inspect(&id).unwrap().runs[0].workspace_id.as_deref(),
        Some("workspace")
    );
}

#[test]
fn failed_due_completion_write_keeps_active_state_and_retries() {
    let (mut engine, store) = fixture();
    let id = create(&mut engine);
    engine
        .request(
            "schedule.update.request",
            json!({"scheduleId":id,"expiresAt":now()}),
            now(),
        )
        .unwrap();
    store.0.lock().unwrap().1 = true;
    assert_eq!(engine.due(now()).unwrap_err(), Error::Storage);
    assert_eq!(engine.inspect(&id).unwrap().status, Status::Active);
    store.0.lock().unwrap().1 = false;
    assert!(engine.due(now()).unwrap().is_empty());
    assert_eq!(engine.inspect(&id).unwrap().status, Status::Completed);
}

#[test]
fn recovery_persistence_failure_refuses_to_open_without_mutating_disk() {
    let (mut engine, store) = fixture();
    let id = create(&mut engine);
    engine.begin(&id, false, now()).unwrap();
    store.0.lock().unwrap().1 = true;
    assert!(matches!(
        Engine::open(
            Box::new(store.clone()),
            now() + chrono::Duration::minutes(2)
        ),
        Err(Error::Storage)
    ));
    assert_eq!(
        store.0.lock().unwrap().0[0].runs[0].status,
        RunStatus::Running
    );
    store.0.lock().unwrap().1 = false;
    let recovered = Engine::open(Box::new(store), now() + chrono::Duration::minutes(2)).unwrap();
    assert_eq!(
        recovered.inspect(&id).unwrap().runs[0].status,
        RunStatus::Failed
    );
}

#[test]
fn restart_keeps_paused_state_and_preserves_previously_finished_history() {
    let (mut engine, store) = fixture();
    let id = create(&mut engine);
    let (_, first) = engine.begin(&id, true, now()).unwrap();
    engine
        .finish(
            &id,
            &first,
            true,
            Outcome {
                output: Some("settled".into()),
                ..Outcome::default()
            },
            now(),
        )
        .unwrap();
    engine
        .request("schedule.pause.request", json!({"scheduleId":id}), now())
        .unwrap();
    engine.begin(&id, true, now()).unwrap();
    let recovered = Engine::open(Box::new(store), now() + chrono::Duration::hours(1)).unwrap();
    let record = recovered.inspect(&id).unwrap();
    assert_eq!(record.status, Status::Paused);
    assert_eq!(record.next_run_at, None);
    assert_eq!(record.runs[0].status, RunStatus::Succeeded);
    assert_eq!(record.runs[0].output.as_deref(), Some("settled"));
    assert_eq!(record.runs[1].status, RunStatus::Failed);
    assert_eq!(
        record.runs[1].ended_at,
        Some(now() + chrono::Duration::hours(1))
    );
}

#[test]
fn clean_restart_does_not_require_a_write_or_change_future_slots() {
    let (mut engine, store) = fixture();
    let id = create(&mut engine);
    engine
        .request("schedule.resume.request", json!({"scheduleId":id}), now())
        .unwrap();
    let mut record = engine.inspect(&id).unwrap();
    record.next_run_at = Some(now() + chrono::Duration::minutes(10));
    store.0.lock().unwrap().0[0] = record.clone();
    store.0.lock().unwrap().1 = true;
    let recovered = Engine::open(Box::new(store), now()).unwrap();
    assert_eq!(json!(recovered.inspect(&id).unwrap()), json!(record));
}

#[test]
fn duplicate_or_invalid_schedule_ids_are_rejected_before_recovery_writes() {
    let (mut engine, store) = fixture();
    let id = create(&mut engine);
    let record = engine.inspect(&id).unwrap();
    store.0.lock().unwrap().0.push(record.clone());
    assert!(matches!(
        Engine::open(Box::new(store.clone()), now()),
        Err(Error::Storage)
    ));
    assert_eq!(store.0.lock().unwrap().0.len(), 2);
    let mut bad = record;
    bad.id = "invalid".into();
    store.0.lock().unwrap().0 = vec![bad];
    assert!(matches!(
        Engine::open(Box::new(store), now()),
        Err(Error::Storage)
    ));
}

#[test]
fn schedule_capacity_refuses_new_admission_without_overwriting_existing_records() {
    let (mut engine, store) = fixture();
    let id = create(&mut engine);
    let template = engine.inspect(&id).unwrap();
    let records: Vec<_> = (0..1024)
        .map(|_| {
            let mut record = template.clone();
            record.id = Uuid::new_v4().to_string();
            record.next_run_at = Some(now() + chrono::Duration::hours(1));
            record
        })
        .collect();
    store.0.lock().unwrap().0.clone_from(&records);
    let mut full = Engine::open(Box::new(store.clone()), now()).unwrap();
    assert_eq!(
        full.request("schedule.create.request", input(), now())
            .unwrap_err(),
        Error::Conflict
    );
    assert_eq!(store.0.lock().unwrap().0.len(), 1024);
    let mut extra = template;
    extra.id = Uuid::new_v4().to_string();
    store.0.lock().unwrap().0.push(extra);
    assert!(matches!(
        Engine::open(Box::new(store), now()),
        Err(Error::Storage)
    ));
}
