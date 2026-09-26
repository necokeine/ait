use super::*;
use serde_json::json;

#[test]
fn notes_deduplicate_replay_and_retain_only_anchors_remaining_after_rewind() {
    let mut notes = Notes::default();
    let entry = NativeItem {
        key: "native:control:one".into(),
        turn_id: Some("turn".into()),
        timestamp: "2026-09-26T00:00:00Z".into(),
        item: json!({"type":"notification","level":"info","message":"Goal paused"}),
    };
    notes.push(entry.clone()).unwrap();
    notes.push(entry.clone()).unwrap();
    let mut notes = Notes::restore(notes.saved().as_ref()).unwrap();
    let mut entries = Vec::new();
    notes.history(&mut entries);
    notes.history(&mut entries);
    assert_eq!(entries.len(), 1);
    notes.retain(&entries);
    assert!(notes.saved().is_some());
    notes.retain(&[]);
    assert!(notes.saved().is_none());
    assert!(Notes::restore(Some(&json!({}))).is_err());
}

#[test]
fn recovered_results_keep_their_position_before_later_native_messages() {
    let item = |key: &str, timestamp: &str| NativeItem {
        key: key.to_owned(),
        turn_id: None,
        timestamp: timestamp.to_owned(),
        item: json!({"type":"assistant_message","text":key}),
    };
    let mut entries = vec![
        item("first", "2026-09-26T00:00:00Z"),
        item("later", "2026-09-26T00:00:02Z"),
    ];
    let mut notes = Notes::default();
    notes
        .push(item("result", "2026-09-26T08:00:01+08:00"))
        .unwrap();
    notes.history(&mut entries);
    notes.history(&mut entries);
    assert_eq!(
        entries
            .iter()
            .map(|entry| entry.key.as_str())
            .collect::<Vec<_>>(),
        ["first", "result", "later"]
    );
}
