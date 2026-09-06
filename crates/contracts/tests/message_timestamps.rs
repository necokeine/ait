//! Compatibility of immutable message timestamps in snapshots and archives.
use ait_contracts::MessageView;
use serde_json::json;

#[test]
fn old_messages_keep_unknown_time_and_new_timestamps_round_trip() {
    let mut record = json!({
        "id": "message", "project_id": "project", "parent_message_id": null,
        "role": "user", "kind": "standard", "text": "hello"
    });
    let legacy: MessageView = serde_json::from_value(record.clone()).unwrap();
    assert_eq!(legacy.created_at, 0);
    record["created_at"] = json!(1_788_743_262_123_i64);
    let current: MessageView = serde_json::from_value(record.clone()).unwrap();
    assert_eq!(current.created_at, 1_788_743_262_123);
    assert_eq!(serde_json::to_value(current).unwrap(), record);
}
