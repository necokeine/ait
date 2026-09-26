use super::*;
use crate::{
    engine::tests::{Memory, input},
    ports::Progress,
    protocol::Schedule,
};
use std::{
    future::Future,
    pin::Pin,
    sync::atomic::{AtomicUsize, Ordering},
};
#[derive(Debug, Default)]
struct Fake {
    calls: AtomicUsize,
    hold: bool,
    panic: bool,
}
impl Runner for Fake {
    fn run(
        &self,
        _: Schedule,
        _: String,
        progress: Progress,
        cancel: CancellationToken,
    ) -> Pin<Box<dyn Future<Output = Outcome> + Send + '_>> {
        Box::pin(async move {
            self.calls.fetch_add(1, Ordering::SeqCst);
            assert!(!self.panic, "runner panic fixture");
            progress
                .record(Some("agent".into()), Some("workspace".into()))
                .await
                .unwrap();
            if self.hold {
                cancel.cancelled().await;
                return Outcome {
                    error: Some("canceled".into()),
                    ..Outcome::default()
                };
            }
            Outcome {
                output: Some("done".into()),
                ..Outcome::default()
            }
        })
    }
}
#[tokio::test]
async fn automatic_run_persists_output_and_completion() {
    let runner = Arc::new(Fake::default());
    let service = Schedules::spawn(Box::new(Memory::default()), runner.clone()).unwrap();
    let mut params = input();
    params["maxRuns"] = json!(1);
    let id = service
        .execute("schedule.create.request", params)
        .await
        .unwrap()["schedule"]["id"]
        .clone();
    let result = tokio::time::timeout(Duration::from_secs(4), async {
        loop {
            let record = service
                .execute("schedule.inspect.request", json!({"scheduleId":id}))
                .await
                .unwrap();
            if record["schedule"]["status"] == "completed" {
                break record;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(result["schedule"]["runs"][0]["output"], "done");
    assert_eq!(runner.calls.load(Ordering::SeqCst), 1);
    service.shutdown().await.unwrap();
    service.shutdown().await.unwrap();
    assert!(
        service
            .execute("schedule.list.request", json!({}))
            .await
            .is_err()
    );
}
#[tokio::test]
async fn manual_run_does_not_hold_lane_and_shutdown_cancels_it() {
    let runner = Arc::new(Fake {
        hold: true,
        ..Fake::default()
    });
    let store = Memory::default();
    let service = Schedules::spawn(Box::new(store.clone()), runner.clone()).unwrap();
    let mut params = input();
    params["runOnCreate"] = json!(false);
    let id = service
        .execute("schedule.create.request", params)
        .await
        .unwrap()["schedule"]["id"]
        .clone();
    let client = service.clone();
    let run = tokio::spawn(async move {
        client
            .execute("schedule.run_once.request", json!({"scheduleId":id}))
            .await
    });
    tokio::time::timeout(Duration::from_secs(2), async {
        while runner.calls.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        service
            .execute("schedule.list.request", json!({}))
            .await
            .unwrap()["schedules"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    tokio::time::sleep(Duration::from_millis(20)).await;
    service.shutdown().await.unwrap();
    assert!(run.await.unwrap().is_ok());
    assert_eq!(
        store.0.lock().unwrap().0[0].runs[0].status,
        crate::protocol::RunStatus::Failed
    );
}
#[tokio::test]
async fn panic_is_recorded_as_failed_instead_of_losing_running_state() {
    let service = Schedules::spawn(
        Box::new(Memory::default()),
        Arc::new(Fake {
            panic: true,
            ..Fake::default()
        }),
    )
    .unwrap();
    let mut params = input();
    params["runOnCreate"] = json!(false);
    let id = service
        .execute("schedule.create.request", params)
        .await
        .unwrap()["schedule"]["id"]
        .clone();
    let result = service
        .execute("schedule.run_once.request", json!({"scheduleId":id}))
        .await
        .unwrap();
    assert_eq!(result["schedule"]["runs"][0]["status"], "failed");
    service.shutdown().await.unwrap();
}

mod concurrency;
mod controlled;
mod durability;
