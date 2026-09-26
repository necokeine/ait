use super::*;
use serde_json::json;

mod paseo;

fn entry(text: &str) -> NativeItem {
    NativeItem {
        key: "native:t:a".into(),
        turn_id: Some("t".into()),
        timestamp: "2026-09-25T00:00:00Z".into(),
        item: json!({"type":"assistant_message","messageId":"a","text":text}),
    }
}

#[test]
fn durable_deltas_share_cursors_and_completion_publishes_only_suffix() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("timeline.sqlite");
    let timeline = Timeline::open(&path).unwrap();
    timeline
        .progress("agent", "codex", "one", &entry("Hello "))
        .unwrap();
    timeline
        .progress("agent", "codex", "one", &entry("Hello "))
        .unwrap();
    assert_eq!(
        timeline.progress("agent", "codex", "one", &entry("changed")),
        Err(ErrorCode::IdempotencyConflict)
    );
    timeline
        .progress("agent", "codex", "two", &entry("world"))
        .unwrap();
    let (epoch, seq) = timeline
        .append("agent", "codex", &[entry("Hello world!")])
        .unwrap();
    assert_eq!(seq, [3]);
    drop(timeline);
    let timeline = Timeline::open(&path).unwrap();
    let (_, rows) = timeline.read("agent").unwrap();
    assert_eq!(
        rows.iter()
            .map(|row| row.entry.item["text"].as_str().unwrap())
            .collect::<String>(),
        "Hello world!"
    );
    assert_eq!(
        rows.iter().map(|row| row.seq).collect::<Vec<_>>(),
        [1, 2, 3]
    );
    assert_eq!(
        timeline
            .append("agent", "codex", &[entry("Hello world!")])
            .unwrap()
            .1,
        [3]
    );
    assert_eq!(
        timeline
            .reconcile("agent", "codex", &[entry("Hello world!")])
            .unwrap(),
        epoch
    );
    assert_eq!(
        timeline.progress("agent", "codex", "late", &entry("late")),
        Err(ErrorCode::IdempotencyConflict)
    );
    let replacement = timeline
        .reconcile("agent", "codex", &[entry("corrected")])
        .unwrap();
    assert_ne!(replacement, epoch);
    let (_, rows) = timeline.read("agent").unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].entry.item["text"], "corrected");
    let database = timeline.database.lock().unwrap();
    assert_eq!(
        database
            .query_row("SELECT count(*) FROM retired_entries", [], |row| row
                .get::<_, u64>(0))
            .unwrap(),
        3
    );
}

#[test]
fn partial_recovery_and_native_divergence_do_not_silently_duplicate_text() {
    let timeline = Timeline::memory().unwrap();
    timeline
        .progress("a", "codex", "partial", &entry("old"))
        .unwrap();
    assert_eq!(
        timeline.append("a", "codex", &[entry("changed")]),
        Err(ErrorCode::IdempotencyConflict)
    );
    assert_eq!(timeline.read("a").unwrap().1.len(), 1);
    timeline
        .reconcile("a", "codex", &[entry("changed")])
        .unwrap();
    assert_eq!(
        timeline.read("a").unwrap().1[0].entry.item["text"],
        "changed"
    );
    // A native history that omits an interrupted partial item retires the provisional rows.
    timeline.reconcile("a", "codex", &[]).unwrap();
    assert!(timeline.read("a").unwrap().1.is_empty());
    timeline
        .progress("a", "codex", "next", &entry("partial"))
        .unwrap();
    timeline
        .append("a", "codex", &[entry("partial recovered")])
        .unwrap();
    assert_eq!(
        timeline.read("a").unwrap().1[1].entry.item["text"],
        " recovered"
    );
}

#[test]
fn version_two_migration_preserves_data_and_progress_failure_is_atomic() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("timeline.sqlite");
    let timeline = Timeline::open(&path).unwrap();
    let (original, _) = timeline.append("a", "codex", &[entry("old")]).unwrap();
    timeline
        .database
        .lock()
        .unwrap()
        .execute_batch("DROP TABLE progress; DROP TABLE input_receipts; DROP TABLE provider_subagents; PRAGMA user_version=2;")
        .unwrap();
    drop(timeline);
    let timeline = Timeline::open(&path).unwrap();
    assert_eq!(timeline.read("a").unwrap().0, original);
    let (outbound, mut receiver) = server_model::outbound::Outbound::new();
    let observer =
        timeline
            .events()
            .subscribe("sub".to_owned(), BTreeSet::from(["b".to_owned()]), outbound);
    observer.activate().unwrap();
    timeline.database.lock().unwrap().execute_batch(
        "CREATE TRIGGER fail_progress BEFORE INSERT ON progress BEGIN SELECT RAISE(ABORT,'fixture'); END;"
    ).unwrap();
    assert_eq!(
        timeline.progress("b", "codex", "one", &entry("new")),
        Err(ErrorCode::AgentIo)
    );
    assert!(receiver.try_recv().is_err());
    assert!(timeline.read("b").unwrap().1.is_empty());
    timeline
        .database
        .lock()
        .unwrap()
        .execute_batch("DROP TRIGGER fail_progress")
        .unwrap();
    timeline
        .progress("b", "codex", "one", &entry("new"))
        .unwrap();
    assert!(receiver.try_recv().is_ok());
    timeline
        .progress("b", "codex", "one", &entry("new"))
        .unwrap();
    assert!(receiver.try_recv().is_err());
    assert_eq!(timeline.read("b").unwrap().1[0].seq, 1);
}

#[test]
fn invalid_progress_and_oversized_fragments_are_rejected() {
    let timeline = Timeline::memory().unwrap();
    let mut invalid = entry("bad");
    invalid.item["type"] = json!("user_message");
    assert_eq!(
        timeline.progress("a", "codex", "id", &invalid),
        Err(ErrorCode::InvalidMessage)
    );
    assert_eq!(
        timeline.progress("a", "codex", "", &entry("x")),
        Err(ErrorCode::InvalidMessage)
    );
    assert_eq!(
        timeline.progress("a", "codex", "large", &entry(&"x".repeat(256 * 1024))),
        Err(ErrorCode::ResourceExhausted)
    );
}
