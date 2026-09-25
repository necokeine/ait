use super::*;

fn item(key: &str, text: &str) -> NativeItem {
    NativeItem {
        key: key.to_owned(),
        turn_id: Some("turn".to_owned()),
        timestamp: "2026-09-25T00:00:00Z".to_owned(),
        item: json!({"type":"user_message","text":text}),
    }
}

#[test]
fn committed_rows_and_epoch_survive_restart_and_replay_is_immutable() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("timeline.sqlite3");
    let timeline = Timeline::open(&path).unwrap();
    let (epoch, seq) = timeline
        .append(
            "agent",
            "codex",
            &[item("one", "hello"), item("two", "world")],
        )
        .unwrap();
    assert_eq!(seq, vec![1, 2]);
    let mut replay = item("one", "hello");
    replay.timestamp = "later".to_owned();
    assert_eq!(
        timeline.append("agent", "codex", &[replay]).unwrap(),
        (epoch.clone(), vec![1])
    );
    assert_eq!(
        timeline.append(
            "agent",
            "codex",
            &[item("three", "uncommitted"), item("one", "changed")]
        ),
        Err(ErrorCode::IdempotencyConflict)
    );
    drop(timeline);
    let timeline = Timeline::open(&path).unwrap();
    let (loaded, rows) = timeline.read("agent").unwrap();
    assert_eq!(loaded, epoch);
    assert_eq!(rows.len(), 2);
    assert_eq!(
        rows[0].value()["sourceSeqRanges"],
        json!([{"startSeq":1,"endSeq":1}])
    );
    assert!(timeline.read("other").unwrap().1.is_empty());
}

#[test]
fn storage_errors_and_oversized_items_do_not_publish_uncommitted_rows() {
    let timeline = Timeline::memory().unwrap();
    let (outbound, mut receiver) = server_model::outbound::Outbound::new();
    let subscription = timeline.events().subscribe(
        "sub".to_owned(),
        std::collections::BTreeSet::from(["agent".to_owned()]),
        outbound,
    );
    subscription.activate().unwrap();
    assert_eq!(
        timeline.append("agent", "codex", &[item("large", &"x".repeat(300_000))]),
        Err(ErrorCode::ResourceExhausted)
    );
    assert!(receiver.try_recv().is_err());
    timeline
        .append("agent", "codex", &[item("one", "ok")])
        .unwrap();
    assert!(receiver.try_recv().is_ok());
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("bad");
    std::fs::write(&path, b"invalid").unwrap();
    assert!(Timeline::open(&path).is_err());
    let future = root.path().join("future");
    Connection::open(&future)
        .unwrap()
        .execute_batch("PRAGMA user_version=99")
        .unwrap();
    assert!(matches!(
        Timeline::open(&future),
        Err(ErrorCode::UnsupportedFormat)
    ));
}

#[test]
fn foreign_sqlite_and_linked_files_are_not_mutated() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("foreign");
    let connection = Connection::open(&path).unwrap();
    connection
        .execute_batch(
            "CREATE TABLE foreign_data(value TEXT); INSERT INTO foreign_data VALUES('keep');",
        )
        .unwrap();
    drop(connection);
    assert!(matches!(
        Timeline::open(&path),
        Err(ErrorCode::UnsupportedFormat)
    ));
    let connection = Connection::open(&path).unwrap();
    assert_eq!(
        connection
            .query_row("SELECT value FROM foreign_data", [], |row| row
                .get::<_, String>(0))
            .unwrap(),
        "keep"
    );
    #[cfg(unix)]
    {
        let link = root.path().join("linked");
        std::os::unix::fs::symlink(&path, &link).unwrap();
        assert!(Timeline::open(&link).is_err());
        let clean = root.path().join("clean");
        std::os::unix::fs::symlink(&path, root.path().join("clean-wal")).unwrap();
        assert!(Timeline::open(&clean).is_err());
    }
}

#[test]
fn refresh_preserves_prefix_cursors_and_retires_rewritten_history_atomically() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("timeline.sqlite3");
    let timeline = Timeline::open(&path).unwrap();
    let first = item("one", "first");
    let second = item("two", "second");
    let epoch = timeline
        .reconcile("agent", "codex", std::slice::from_ref(&first))
        .unwrap();
    let mut plugin = item("plugin:card", "");
    plugin.item = json!({"type":"plugin","data":{"keep":true}});
    timeline.append("agent", "codex", &[plugin]).unwrap();
    assert_eq!(
        timeline
            .reconcile("agent", "codex", &[first, second.clone()])
            .unwrap(),
        epoch
    );
    let (outbound, mut receiver) = server_model::outbound::Outbound::new();
    let observer = timeline.events().subscribe(
        "sub".to_owned(),
        std::collections::BTreeSet::from(["agent".to_owned()]),
        outbound,
    );
    observer.activate().unwrap();
    assert_eq!(
        timeline.reconcile("agent", "codex", &[item("large", &"x".repeat(300_000))]),
        Err(ErrorCode::ResourceExhausted)
    );
    assert_eq!(timeline.read("agent").unwrap().0, epoch);
    assert!(receiver.try_recv().is_err());
    let replacement = timeline
        .reconcile("agent", "codex", &[item("one", "rewritten")])
        .unwrap();
    assert_ne!(replacement, epoch);
    assert!(receiver.try_recv().is_ok());
    let (_, rows) = timeline.read("agent").unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].entry.item["text"], "rewritten");
    assert_eq!(rows[1].entry.item["data"]["keep"], true);
    let count: usize = timeline
        .database
        .lock()
        .unwrap()
        .query_row(
            "SELECT count(*) FROM retired_entries WHERE epoch=?",
            [epoch],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 3);
    drop(timeline);
    let timeline = Timeline::open(&path).unwrap();
    assert_eq!(timeline.read("agent").unwrap().0, replacement);
    let empty = timeline.reconcile("agent", "codex", &[]).unwrap();
    assert_ne!(empty, replacement);
    assert_eq!(timeline.read("agent").unwrap().1.len(), 1);
    assert_eq!(timeline.reconcile("agent", "codex", &[]).unwrap(), empty);
}

#[test]
fn version_one_migration_keeps_existing_rows_and_epoch() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("timeline.sqlite3");
    let database = Connection::open(&path).unwrap();
    database.execute_batch("CREATE TABLE timelines(agent TEXT PRIMARY KEY, epoch TEXT NOT NULL);
        CREATE TABLE entries(agent TEXT NOT NULL, seq INTEGER NOT NULL, identity TEXT NOT NULL,
        provider TEXT NOT NULL, entry TEXT NOT NULL, PRIMARY KEY(agent,seq), UNIQUE(agent,identity));
        INSERT INTO timelines VALUES('agent','original'); PRAGMA user_version=1;
        PRAGMA application_id=1096045644;").unwrap();
    database
        .execute(
            "INSERT INTO entries VALUES('agent',1,'one','codex',?)",
            [serde_json::to_string(&item("one", "original")).unwrap()],
        )
        .unwrap();
    drop(database);
    let migrated = Timeline::open(&path).unwrap();
    let (epoch, rows) = migrated.read("agent").unwrap();
    assert_eq!(epoch, "original");
    assert_eq!(rows[0].entry.item["text"], "original");
    assert_ne!(migrated.reconcile("agent", "codex", &[]).unwrap(), epoch);
}
