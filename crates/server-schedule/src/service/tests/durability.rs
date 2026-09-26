//! Failure injection around Paseo's durable run history and concurrent mutation scenarios.
use super::{controlled::*, *};
use crate::{ports::Store, protocol::RunStatus};
use std::sync::atomic::AtomicBool;

#[derive(Debug, Clone, Default)]
struct FaultStore {
    records: Arc<Mutex<Vec<Schedule>>>,
    failing: Arc<AtomicBool>,
    failures: Arc<AtomicUsize>,
    terminal_only: Arc<AtomicBool>,
    failed: Arc<tokio::sync::Notify>,
}

impl Store for FaultStore {
    fn load(&self) -> Result<Vec<Schedule>, Error> {
        Ok(self.records.lock().unwrap().clone())
    }
    fn save(&mut self, records: &[Schedule]) -> Result<(), Error> {
        if self.failing.load(Ordering::SeqCst)
            && (!self.terminal_only.load(Ordering::SeqCst)
                || records.iter().any(|schedule| {
                    schedule
                        .runs
                        .iter()
                        .any(|run| run.status != RunStatus::Running)
                }))
        {
            self.failures.fetch_add(1, Ordering::SeqCst);
            self.failed.notify_one();
            return Err(Error::Storage);
        }
        *self.records.lock().unwrap() = records.to_vec();
        Ok(())
    }
}

#[tokio::test]
async fn failed_settlement_retries_persistence_without_repeating_runner_side_effects() {
    let (runner, mut receive) = Controlled::new();
    let store = FaultStore::default();
    let service = Schedules::spawn(Box::new(store.clone()), runner.clone()).unwrap();
    let id = create(&service).await;
    let pending = run(&service, &id);
    let accepted = started(&mut receive).await;
    accepted
        .progress
        .record(Some("agent".into()), Some("workspace".into()))
        .await
        .unwrap();
    store.failing.store(true, Ordering::SeqCst);
    accepted
        .complete
        .send(Outcome {
            output: Some("committed once".into()),
            ..Outcome::default()
        })
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), store.failed.notified())
        .await
        .unwrap();
    assert!(!pending.is_finished());
    assert_eq!(
        store.records.lock().unwrap()[0].runs[0].status,
        RunStatus::Running
    );
    let visible = service
        .execute("schedule.inspect.request", json!({"scheduleId":id}))
        .await
        .unwrap();
    assert_eq!(visible["schedule"]["runs"][0]["status"], "running");
    assert_eq!(
        settled(run(&service, &id)).await.unwrap_err(),
        Error::Storage
    );
    store.failing.store(false, Ordering::SeqCst);
    let result = settled(pending).await.unwrap();
    assert_eq!(result["schedule"]["runs"][0]["output"], "committed once");
    assert_eq!(result["schedule"]["runs"][0]["workspaceId"], "workspace");
    assert_eq!(runner.calls.load(Ordering::SeqCst), 1);
    assert_eq!(store.records.lock().unwrap()[0].runs.len(), 1);
    assert!(store.failures.load(Ordering::SeqCst) >= 1);
    service.shutdown().await.unwrap();
}

#[tokio::test]
async fn settlement_retry_merges_edits_made_after_the_first_failed_write() {
    let (runner, mut receive) = Controlled::new();
    let store = FaultStore::default();
    let service = Schedules::spawn(Box::new(store.clone()), runner).unwrap();
    let id = create(&service).await;
    let pending = run(&service, &id);
    let accepted = started(&mut receive).await;
    store.terminal_only.store(true, Ordering::SeqCst);
    store.failing.store(true, Ordering::SeqCst);
    accepted
        .complete
        .send(Outcome {
            output: Some("done".into()),
            ..Outcome::default()
        })
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), store.failed.notified())
        .await
        .unwrap();
    service
        .execute("schedule.pause.request", json!({"scheduleId":id}))
        .await
        .unwrap();
    service
        .execute(
            "schedule.update.request",
            json!({"scheduleId":id,"prompt":"new prompt"}),
        )
        .await
        .unwrap();
    store.failing.store(false, Ordering::SeqCst);
    let result = settled(pending).await.unwrap();
    assert_eq!(result["schedule"]["status"], "paused");
    assert_eq!(result["schedule"]["prompt"], "new prompt");
    assert_eq!(result["schedule"]["runs"][0]["status"], "succeeded");
    assert_eq!(result["schedule"]["runs"][0]["output"], "done");
    service.shutdown().await.unwrap();
}

#[tokio::test]
async fn failed_begin_write_never_invokes_the_runner_and_retry_admits_once() {
    let (runner, mut receive) = Controlled::new();
    let store = FaultStore::default();
    let service = Schedules::spawn(Box::new(store.clone()), runner.clone()).unwrap();
    let id = create(&service).await;
    store.failing.store(true, Ordering::SeqCst);
    assert_eq!(
        settled(run(&service, &id)).await.unwrap_err(),
        Error::Storage
    );
    assert_eq!(runner.calls.load(Ordering::SeqCst), 0);
    assert!(store.records.lock().unwrap()[0].runs.is_empty());
    store.failing.store(false, Ordering::SeqCst);
    let pending = run(&service, &id);
    started(&mut receive)
        .await
        .complete
        .send(Outcome::default())
        .unwrap();
    settled(pending).await.unwrap();
    assert_eq!(runner.calls.load(Ordering::SeqCst), 1);
    service.shutdown().await.unwrap();
}

#[tokio::test]
async fn checkpoint_failure_is_reported_to_runner_without_publishing_allocated_identity() {
    let (runner, mut receive) = Controlled::new();
    let store = FaultStore::default();
    let service = Schedules::spawn(Box::new(store.clone()), runner).unwrap();
    let id = create(&service).await;
    let pending = run(&service, &id);
    let accepted = started(&mut receive).await;
    store.failing.store(true, Ordering::SeqCst);
    assert_eq!(
        accepted
            .progress
            .record(Some("agent".into()), Some("workspace".into()))
            .await
            .unwrap_err(),
        Error::Storage
    );
    assert_eq!(store.records.lock().unwrap()[0].runs[0].agent_id, None);
    store.failing.store(false, Ordering::SeqCst);
    accepted
        .complete
        .send(Outcome {
            error: Some("allocation cleaned up".into()),
            ..Outcome::default()
        })
        .unwrap();
    let result = settled(pending).await.unwrap();
    assert_eq!(result["schedule"]["runs"][0]["status"], "failed");
    assert_eq!(result["schedule"]["runs"][0]["agentId"], Value::Null);
    service.shutdown().await.unwrap();
}

#[tokio::test]
async fn shutdown_with_failed_settlement_leaves_running_record_for_restart_recovery() {
    let (runner, mut receive) = Controlled::new();
    let store = FaultStore::default();
    let service = Schedules::spawn(Box::new(store.clone()), runner.clone()).unwrap();
    let id = create(&service).await;
    let pending = run(&service, &id);
    let accepted = started(&mut receive).await;
    accepted
        .progress
        .record(None, Some("workspace".into()))
        .await
        .unwrap();
    store.failing.store(true, Ordering::SeqCst);
    accepted.complete.send(Outcome::default()).unwrap();
    tokio::time::timeout(Duration::from_secs(5), store.failed.notified())
        .await
        .unwrap();
    service.shutdown().await.unwrap();
    assert_eq!(settled(pending).await.unwrap_err(), Error::Storage);
    assert_eq!(
        store.records.lock().unwrap()[0].runs[0].status,
        RunStatus::Running
    );
    store.failing.store(false, Ordering::SeqCst);
    let restarted = Schedules::spawn(Box::new(store.clone()), runner.clone()).unwrap();
    let recovered = restarted
        .execute("schedule.inspect.request", json!({"scheduleId":id}))
        .await
        .unwrap();
    assert_eq!(recovered["schedule"]["runs"][0]["status"], "failed");
    assert_eq!(recovered["schedule"]["runs"][0]["workspaceId"], "workspace");
    assert_eq!(runner.calls.load(Ordering::SeqCst), 1);
    restarted.shutdown().await.unwrap();
}
