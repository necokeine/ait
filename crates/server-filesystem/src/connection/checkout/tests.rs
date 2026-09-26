use std::future::{Future, poll_fn};
use std::pin::Pin;
use std::task::Poll;
use std::time::Duration;

use super::{CheckoutDiffSubscription, poll_diff};

mod fixtures;

use fixtures::Harness;

async fn assert_waiting(mut future: Pin<&mut impl Future>) {
    poll_fn(|context| {
        assert!(future.as_mut().poll(context).is_pending());
        Poll::Ready(())
    })
    .await;
}

#[tokio::test]
async fn contended_diff_polls_all_progress_in_arrival_order() {
    let mut harness = Harness::new();
    let observations = [
        harness.observation("first"),
        harness.observation("second"),
        harness.observation("third"),
    ];
    harness.arm();
    let held = harness.jobs.clone().acquire_owned().await.unwrap();
    let cancellation = tokio_util::sync::CancellationToken::new();
    let polls = observations.map(|observation| {
        let service = harness.service.clone();
        let jobs = harness.jobs.clone();
        let cancellation = cancellation.clone();
        let server_cancel = harness.server_cancel.clone();
        Box::pin(async move {
            poll_diff(service, jobs, &observation, &cancellation, &server_cancel).await
        })
    });
    let [mut first, mut second, mut third] = polls;
    assert_waiting(first.as_mut()).await;
    assert_waiting(second.as_mut()).await;
    assert_waiting(third.as_mut()).await;
    harness.assert_no_calls();
    drop(held);

    let drive = async {
        for expected in ["first", "second", "third"] {
            let call = harness.next_call().await;
            assert_eq!(call.cwd, expected);
            drop(call);
        }
    };
    let (snapshots, ()) = tokio::join!(async { tokio::join!(first, second, third) }, drive);
    for snapshot in [snapshots.0, snapshots.1, snapshots.2] {
        assert!(!snapshot.unwrap().unwrap().files.is_empty());
    }
    assert_eq!(harness.jobs.available_permits(), 1);
}

#[tokio::test]
async fn released_diff_subscription_leaves_the_permit_queue_without_running() {
    let mut harness = Harness::new();
    let observation = harness.observation("released");
    harness.arm();
    let held = harness.jobs.clone().acquire_owned().await.unwrap();
    let subscription = CheckoutDiffSubscription {
        cancellation: tokio_util::sync::CancellationToken::new(),
    };
    let cancellation = subscription.cancellation.clone();
    let mut poll = Box::pin(poll_diff(
        harness.service.clone(),
        harness.jobs.clone(),
        &observation,
        &cancellation,
        &harness.server_cancel,
    ));
    assert_waiting(poll.as_mut()).await;
    drop(subscription);
    assert!(
        tokio::time::timeout(Duration::from_secs(5), poll)
            .await
            .expect("subscription release interrupts the permit wait")
            .is_none()
    );
    harness.assert_no_calls();
    drop(held);
    assert!(harness.jobs.clone().try_acquire_owned().is_ok());
}

#[tokio::test]
async fn server_shutdown_removes_a_queued_diff_poll_without_running() {
    let mut harness = Harness::new();
    let observation = harness.observation("shutdown");
    harness.arm();
    let held = harness.jobs.clone().acquire_owned().await.unwrap();
    let cancellation = tokio_util::sync::CancellationToken::new();
    let mut poll = Box::pin(poll_diff(
        harness.service.clone(),
        harness.jobs.clone(),
        &observation,
        &cancellation,
        &harness.server_cancel,
    ));
    assert_waiting(poll.as_mut()).await;
    harness.server_cancel.cancel();
    assert!(
        tokio::time::timeout(Duration::from_secs(5), poll)
            .await
            .expect("server cancellation interrupts the permit wait")
            .is_none()
    );
    harness.assert_no_calls();
    drop(held);
    assert!(harness.jobs.clone().try_acquire_owned().is_ok());
}

#[tokio::test]
async fn releasing_in_flight_diff_keeps_work_tracked_and_suppresses_late_updates() {
    let mut harness = Harness::new();
    let pending = harness.pending("released");
    harness.arm();
    let (_, subscription) = pending.activate();
    let call = harness.next_call().await;
    drop(subscription);
    harness.tracker.close();
    assert!(!harness.tracker.is_empty());
    assert_eq!(harness.jobs.available_permits(), 0);
    drop(call);
    harness.wait_for_tasks().await;
    harness.assert_no_updates();
    assert_eq!(harness.jobs.available_permits(), 1);
}

#[tokio::test]
async fn shutdown_during_diff_keeps_work_tracked_and_suppresses_late_updates() {
    let mut harness = Harness::new();
    let pending = harness.pending("shutdown");
    harness.arm();
    let (_, subscription) = pending.activate();
    let call = harness.next_call().await;
    harness.server_cancel.cancel();
    harness.tracker.close();
    assert!(!harness.tracker.is_empty());
    assert_eq!(harness.jobs.available_permits(), 0);
    drop(call);
    harness.wait_for_tasks().await;
    harness.assert_no_updates();
    assert_eq!(harness.jobs.available_permits(), 1);
    drop(subscription);
}
