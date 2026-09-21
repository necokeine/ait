use super::*;
use serde_json::json;

fn message(length: usize) -> ServerMessage {
    ServerMessage::Response {
        request_id: "1".to_owned(),
        result: json!("x".repeat(length)),
    }
}

#[test]
fn count_budget_rejects_slow_consumer_and_releases_on_drop() {
    let (queue, mut receiver) = Outbound::new();
    for _ in 0..MAX_QUEUE_MESSAGES {
        queue.send(&message(1)).unwrap();
    }
    assert!(matches!(queue.send(&message(1)), Err(QueueError::Full)));
    drop(receiver.try_recv().unwrap());
    queue.send(&message(1)).unwrap();
    drop(receiver);
    assert_eq!(queue.bytes.available_permits(), MAX_QUEUE_BYTES);
    assert!(queue.send(&message(1)).is_err());
}

#[test]
fn byte_budget_includes_in_flight_write_and_returns_its_permits() {
    let (queue, mut receiver) = Outbound::new();
    let large = message(MAX_QUEUE_BYTES / 2);
    queue.send(&large).unwrap();
    let in_flight = receiver.try_recv().unwrap();
    assert!(matches!(queue.send(&large), Err(QueueError::Full)));
    drop(in_flight);
    queue.send(&large).unwrap();
    assert!(queue.send(&message(MAX_QUEUE_BYTES)).is_err());
}
