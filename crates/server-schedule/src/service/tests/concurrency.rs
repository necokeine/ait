//! Actor counterparts of Paseo concurrent updates and in-flight lifecycle tests.
use super::{controlled::*, *};
use crate::protocol::{RunStatus, Status};

#[tokio::test]
async fn global_run_limit_releases_capacity_only_after_an_occurrence_finishes() {
    let (runner, mut receive) = Controlled::new();
    let service = Schedules::spawn(Box::new(Memory::default()), runner.clone()).unwrap();
    let mut running = Vec::new();
    for _ in 0..16 {
        let id = create(&service).await;
        let pending = run(&service, &id);
        running.push((pending, started(&mut receive).await));
    }
    let waiting = create(&service).await;
    assert_eq!(
        settled(run(&service, &waiting)).await.unwrap_err(),
        Error::Conflict
    );
    assert_eq!(runner.calls.load(Ordering::SeqCst), 16);
    let (pending, first) = running.pop().unwrap();
    first.complete.send(Outcome::default()).unwrap();
    settled(pending).await.unwrap();
    let replacement = run(&service, &waiting);
    let accepted = started(&mut receive).await;
    assert_eq!(accepted.schedule.id, waiting.as_str().unwrap());
    accepted.complete.send(Outcome::default()).unwrap();
    settled(replacement).await.unwrap();
    for (pending, started) in running {
        started.complete.send(Outcome::default()).unwrap();
        settled(pending).await.unwrap();
    }
    assert_eq!(runner.calls.load(Ordering::SeqCst), 17);
    service.shutdown().await.unwrap();
}

#[tokio::test]
async fn running_one_schedule_does_not_block_another_or_allow_duplicate_occurrences() {
    let (runner, mut receive) = Controlled::new();
    let service = Schedules::spawn(Box::new(Memory::default()), runner.clone()).unwrap();
    let first = create(&service).await;
    let second = create(&service).await;
    let first_run = run(&service, &first);
    let first_started = started(&mut receive).await;
    assert_eq!(
        settled(run(&service, &first)).await.unwrap_err(),
        Error::Conflict
    );
    let second_run = run(&service, &second);
    let second_started = started(&mut receive).await;
    second_started
        .complete
        .send(Outcome {
            output: Some("second".into()),
            ..Outcome::default()
        })
        .unwrap();
    assert_eq!(
        settled(second_run).await.unwrap()["schedule"]["runs"][0]["output"],
        "second"
    );
    assert!(!first_run.is_finished());
    first_started.complete.send(Outcome::default()).unwrap();
    settled(first_run).await.unwrap();
    assert_eq!(runner.calls.load(Ordering::SeqCst), 2);
    service.shutdown().await.unwrap();
}

#[tokio::test]
async fn concurrent_field_updates_during_execution_keep_checkpoint_and_outcome() {
    let (runner, mut receive) = Controlled::new();
    let store = Memory::default();
    let service = Schedules::spawn(Box::new(store.clone()), runner).unwrap();
    let id = create(&service).await;
    let pending = run(&service, &id);
    let accepted = started(&mut receive).await;
    accepted
        .progress
        .record(Some("agent".into()), Some("workspace".into()))
        .await
        .unwrap();
    let (name, prompt) = tokio::join!(
        service.execute(
            "schedule.update.request",
            json!({"scheduleId":id,"name":"updated name"})
        ),
        service.execute(
            "schedule.update.request",
            json!({"scheduleId":id,"prompt":"updated prompt"})
        )
    );
    name.unwrap();
    prompt.unwrap();
    accepted
        .complete
        .send(Outcome {
            output: Some("executed original prompt".into()),
            ..Outcome::default()
        })
        .unwrap();
    let result = settled(pending).await.unwrap();
    assert_eq!(accepted.schedule.prompt, "hello");
    assert_eq!(result["schedule"]["name"], "updated name");
    assert_eq!(result["schedule"]["prompt"], "updated prompt");
    assert_eq!(result["schedule"]["runs"][0]["agentId"], "agent");
    assert_eq!(result["schedule"]["runs"][0]["workspaceId"], "workspace");
    assert_eq!(
        result["schedule"]["runs"][0]["output"],
        "executed original prompt"
    );
    assert_eq!(result["schedule"], json!(store.0.lock().unwrap().0[0]));
    service.shutdown().await.unwrap();
}

#[tokio::test]
async fn dropped_manual_rpc_waiter_does_not_cancel_an_admitted_run() {
    let (runner, mut receive) = Controlled::new();
    let store = Memory::default();
    let service = Schedules::spawn(Box::new(store.clone()), runner).unwrap();
    let id = create(&service).await;
    let pending = run(&service, &id);
    let accepted = started(&mut receive).await;
    pending.abort();
    assert!(pending.await.unwrap_err().is_cancelled());
    accepted
        .complete
        .send(Outcome {
            output: Some("survives socket".into()),
            ..Outcome::default()
        })
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let result = service
                .execute("schedule.inspect.request", json!({"scheduleId":id}))
                .await
                .unwrap();
            if result["schedule"]["runs"][0]["status"] == "succeeded" {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        store.0.lock().unwrap().0[0].runs[0].output.as_deref(),
        Some("survives socket")
    );
    service.shutdown().await.unwrap();
}

#[tokio::test]
async fn deleting_inflight_schedule_does_not_recreate_it_after_completion() {
    let (runner, mut receive) = Controlled::new();
    let store = Memory::default();
    let service = Schedules::spawn(Box::new(store.clone()), runner).unwrap();
    let id = create(&service).await;
    let pending = run(&service, &id);
    let accepted = started(&mut receive).await;
    service
        .execute("schedule.delete.request", json!({"scheduleId":id}))
        .await
        .unwrap();
    accepted.complete.send(Outcome::default()).unwrap();
    assert_eq!(settled(pending).await.unwrap_err(), Error::NotFound);
    assert!(store.0.lock().unwrap().0.is_empty());
    service.shutdown().await.unwrap();
}

#[tokio::test]
async fn shutdown_settles_all_accepted_runs_and_preserves_a_concurrent_pause() {
    let (runner, mut receive) = Controlled::new();
    let store = Memory::default();
    let service = Schedules::spawn(Box::new(store.clone()), runner).unwrap();
    let first = create(&service).await;
    let second = create(&service).await;
    let first_run = run(&service, &first);
    let _first_started = started(&mut receive).await;
    let second_run = run(&service, &second);
    let _second_started = started(&mut receive).await;
    service
        .execute("schedule.pause.request", json!({"scheduleId":first}))
        .await
        .unwrap();
    service.shutdown().await.unwrap();
    assert_eq!(
        settled(first_run).await.unwrap()["schedule"]["status"],
        "paused"
    );
    settled(second_run).await.unwrap();
    let records = &store.0.lock().unwrap().0;
    assert_eq!(records[0].status, Status::Paused);
    assert!(
        records
            .iter()
            .all(|record| record.runs.len() == 1 && record.runs[0].status == RunStatus::Failed)
    );
}

#[derive(Debug)]
struct CheckpointOnCancel {
    entered: tokio::sync::Notify,
}

impl Runner for CheckpointOnCancel {
    fn run(
        &self,
        _: Schedule,
        _: String,
        progress: Progress,
        cancel: CancellationToken,
    ) -> Pin<Box<dyn Future<Output = Outcome> + Send + '_>> {
        Box::pin(async move {
            self.entered.notify_one();
            cancel.cancelled().await;
            progress
                .record(
                    Some("cleanup-agent".into()),
                    Some("cleanup-workspace".into()),
                )
                .await
                .expect("shutdown must continue acknowledging accepted runner checkpoints");
            Outcome {
                error: Some("cleaned up after cancellation".into()),
                ..Outcome::default()
            }
        })
    }
}

#[tokio::test]
async fn shutdown_drains_checkpoint_acknowledgments_needed_by_runner_cleanup() {
    let runner = Arc::new(CheckpointOnCancel {
        entered: tokio::sync::Notify::new(),
    });
    let store = Memory::default();
    let service = Schedules::spawn(Box::new(store.clone()), runner.clone()).unwrap();
    let id = create(&service).await;
    let pending = run(&service, &id);
    tokio::time::timeout(Duration::from_secs(5), runner.entered.notified())
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(10), service.shutdown())
        .await
        .unwrap()
        .unwrap();
    let result = settled(pending).await.unwrap();
    assert_eq!(result["schedule"]["runs"][0]["status"], "failed");
    assert_eq!(result["schedule"]["runs"][0]["agentId"], "cleanup-agent");
    assert_eq!(
        result["schedule"]["runs"][0]["workspaceId"],
        "cleanup-workspace"
    );
    assert_eq!(
        result["schedule"]["runs"][0]["error"],
        "cleaned up after cancellation"
    );
    assert_eq!(store.0.lock().unwrap().0[0].runs.len(), 1);
}
