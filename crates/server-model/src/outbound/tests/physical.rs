//! Bounded physical-send behavior corresponding to Paseo websocket/physical-socket tests.

use std::future::{Future, poll_fn};
use std::task::Poll;

use super::*;

#[tokio::test]
async fn exact_binary_byte_budget_is_accepted_and_one_extra_byte_is_rejected() {
    let (queue, mut receiver) = Outbound::new();
    queue.binary(vec![7; MAX_QUEUE_BYTES]).await.unwrap();
    assert_eq!(queue.bytes.available_permits(), 0);
    let queued = receiver.recv().await.unwrap();
    assert!(matches!(&queued.message, Frame::Binary(bytes) if bytes.len() == MAX_QUEUE_BYTES));
    drop(queued);
    assert_eq!(queue.bytes.available_permits(), MAX_QUEUE_BYTES);
    assert!(matches!(
        queue.binary(vec![7; MAX_QUEUE_BYTES + 1]).await,
        Err(QueueError::Full)
    ));
    assert!(receiver.try_recv().is_err());
    assert_eq!(queue.bytes.available_permits(), MAX_QUEUE_BYTES);
}

#[tokio::test]
async fn binary_backpressure_waits_until_the_in_flight_frame_is_released() {
    let (queue, mut receiver) = Outbound::new();
    queue.binary(vec![1; MAX_QUEUE_BYTES]).await.unwrap();
    let in_flight = receiver.recv().await.unwrap();
    let mut waiting = std::pin::pin!(queue.binary(b"next".to_vec()));
    poll_fn(|context| {
        assert!(waiting.as_mut().poll(context).is_pending());
        Poll::Ready(())
    })
    .await;
    assert!(receiver.try_recv().is_err());
    drop(in_flight);
    waiting.await.unwrap();
    let next = receiver.recv().await.unwrap();
    assert!(matches!(&next.message, Frame::Binary(bytes) if bytes == b"next"));
}

#[tokio::test]
async fn cancelled_binary_waiter_releases_permits_and_never_enqueues_late_output() {
    let (queue, mut receiver) = Outbound::new();
    queue.binary(vec![1; MAX_QUEUE_BYTES]).await.unwrap();
    let mut waiting = std::pin::pin!(queue.binary(b"late".to_vec()));
    poll_fn(|context| {
        assert!(waiting.as_mut().poll(context).is_pending());
        Poll::Ready(())
    })
    .await;
    queue.failure().cancel();
    assert!(matches!(waiting.await, Err(QueueError::Full)));
    drop(receiver.recv().await.unwrap());
    assert!(receiver.try_recv().is_err());
    assert_eq!(queue.bytes.available_permits(), MAX_QUEUE_BYTES);
}

#[tokio::test]
async fn dropping_a_pending_binary_send_returns_its_reserved_budget() {
    let (queue, mut receiver) = Outbound::new();
    for _ in 0..MAX_QUEUE_MESSAGES {
        queue.send(&message(1)).unwrap();
    }
    let available = queue.bytes.available_permits();
    {
        let mut waiting = std::pin::pin!(queue.binary(vec![5; 1024]));
        poll_fn(|context| {
            assert!(waiting.as_mut().poll(context).is_pending());
            Poll::Ready(())
        })
        .await;
        assert_eq!(queue.bytes.available_permits(), available - 1024);
    }
    assert_eq!(queue.bytes.available_permits(), available);
    for _ in 0..MAX_QUEUE_MESSAGES {
        assert!(matches!(
            receiver.recv().await.unwrap().message,
            Frame::Text(_)
        ));
    }
    assert!(receiver.try_recv().is_err());
    assert_eq!(queue.bytes.available_permits(), MAX_QUEUE_BYTES);
}

#[tokio::test]
async fn closed_transport_rejects_binary_and_returns_the_reserved_bytes() {
    let (queue, receiver) = Outbound::new();
    drop(receiver);
    assert!(matches!(
        queue.binary(vec![0; 1024]).await,
        Err(QueueError::Full)
    ));
    assert_eq!(queue.bytes.available_permits(), MAX_QUEUE_BYTES);
}

#[tokio::test]
async fn binary_and_json_frames_share_budget_and_keep_enqueue_order_across_clones() {
    let (queue, mut receiver) = Outbound::new();
    let sibling = queue.clone();
    queue.send(&message(10)).unwrap();
    sibling.binary(b"middle".to_vec()).await.unwrap();
    queue
        .respond("last".to_owned(), Ok(json!({"done":true})))
        .unwrap();
    assert!(matches!(
        receiver.recv().await.unwrap().message,
        Frame::Text(_)
    ));
    let middle = receiver.recv().await.unwrap();
    assert!(matches!(&middle.message, Frame::Binary(bytes) if bytes == b"middle"));
    let last = receiver.recv().await.unwrap();
    let Frame::Text(text) = &last.message else {
        panic!("expected response")
    };
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(text).unwrap()["request_id"],
        "last"
    );
    drop((middle, last));
    assert_eq!(queue.bytes.available_permits(), MAX_QUEUE_BYTES);
}

#[tokio::test]
async fn text_send_failure_cancels_binary_work_for_every_queue_clone() {
    let (queue, receiver) = Outbound::new();
    let sibling = queue.clone();
    drop(receiver);
    assert!(queue.send(&message(1)).is_err());
    assert!(sibling.failure().is_cancelled());
    assert!(matches!(
        sibling.binary(b"late".to_vec()).await,
        Err(QueueError::Full)
    ));
    assert_eq!(queue.bytes.available_permits(), MAX_QUEUE_BYTES);
}
