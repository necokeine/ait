use std::sync::Arc;

use super::*;

fn record(
    kind: ControlRecordKind,
    id: &str,
    project_id: Option<&str>,
    value: Value,
) -> ControlRecord {
    ControlRecord {
        kind,
        id: id.into(),
        project_id: project_id.map(str::to_owned),
        value,
    }
}

#[tokio::test]
async fn reads_are_bounded_by_entity_and_project() {
    let store = SqliteControlStore::in_memory().unwrap();
    store
        .apply(
            0,
            vec![
                ControlChange::Put(record(
                    ControlRecordKind::Project,
                    "p1",
                    Some("p1"),
                    serde_json::json!({"id":"p1","workdir":"/p1"}),
                )),
                ControlChange::Put(record(
                    ControlRecordKind::Project,
                    "p2",
                    Some("p2"),
                    serde_json::json!({"id":"p2","workdir":"/p2"}),
                )),
                ControlChange::Put(record(
                    ControlRecordKind::Message,
                    "m1",
                    Some("p1"),
                    serde_json::json!({"id":"m1","project_id":"p1","parent_message_id":null}),
                )),
                ControlChange::Put(record(
                    ControlRecordKind::Message,
                    "m2",
                    Some("p2"),
                    serde_json::json!({"id":"m2","project_id":"p2","parent_message_id":null}),
                )),
                ControlChange::Put(record(
                    ControlRecordKind::Message,
                    "m1-child",
                    Some("p1"),
                    serde_json::json!({"id":"m1-child","project_id":"p1","parent_message_id":"m1"}),
                )),
            ],
            Vec::new(),
        )
        .await
        .unwrap();

    let read = store
        .read(&[ControlFilter::project(ControlRecordKind::Message, "p1")])
        .await
        .unwrap();
    assert_eq!(read.records.len(), 2);
    assert_eq!(
        read.records
            .iter()
            .map(|record| record.id.as_str())
            .collect::<Vec<_>>(),
        vec!["m1", "m1-child"]
    );

    let children = store
        .read(&[ControlFilter::message_children("m1")])
        .await
        .unwrap();
    assert_eq!(children.records.len(), 1);
    assert_eq!(children.records[0].id, "m1-child");

    let by_path = store
        .read(&[ControlFilter::ProjectWorkdir {
            workdir: "/p2".into(),
        }])
        .await
        .unwrap();
    assert_eq!(by_path.records.len(), 1);
    assert_eq!(by_path.records[0].id, "p2");
}

#[tokio::test]
async fn immutable_messages_cannot_be_replaced() {
    let store = SqliteControlStore::in_memory().unwrap();
    store
        .apply(
            0,
            vec![
                ControlChange::Put(record(
                    ControlRecordKind::Project,
                    "p1",
                    Some("p1"),
                    serde_json::json!({"id": "p1", "workdir": "/p1"}),
                )),
                ControlChange::Put(record(
                    ControlRecordKind::Message,
                    "m1",
                    Some("p1"),
                    serde_json::json!({
                        "id": "m1",
                        "project_id": "p1",
                        "parent_message_id": null,
                        "text": "one",
                    }),
                )),
            ],
            Vec::new(),
        )
        .await
        .unwrap();
    let failure = store.apply(1, vec![ControlChange::Put(record(
        ControlRecordKind::Message, "m1", Some("p1"),
        serde_json::json!({"id":"m1","project_id":"p1","parent_message_id":null,"text":"two"}),
    ))], Vec::new()).await.unwrap_err();
    assert!(failure.to_string().contains("immutable"));
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn legacy_control_blob_is_split_once_and_removed() {
    let temporary = tempfile::TempDir::new().unwrap();
    let database = temporary.path().join("legacy.sqlite3");
    let legacy = serde_json::json!({
        "projects": [{"id":"p1","workdir":"/p1"}],
        "agents": [
            {
                "id": "a1", "name": "one", "mode": "codex", "model": "gpt-5.6-sol",
                "owner_session_id": null, "revision": 1, "enabled": true,
            },
            {
                "id": "a2", "name": "two", "mode": "codex", "model": "legacy-model",
                "owner_session_id": null, "revision": 1, "enabled": true,
            }
        ],
        "sessions": [],
        "messages": [{"id":"m1","project_id":"p1","parent_message_id":null}],
        "runs": [],
        "crons": [],
        "provider_credentials": {},
        "run_credentials": {},
        "workspace_run_journals": {},
        "settings": {},
        "settings_revision": 3,
    });
    let connection = Connection::open(&database).unwrap();
    connection
        .execute_batch(
            "CREATE TABLE control_state(
                singleton INTEGER PRIMARY KEY,
                revision INTEGER NOT NULL,
                body_json TEXT NOT NULL
            );",
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO control_state(singleton, revision, body_json) VALUES(1, 7, ?1)",
            [legacy.to_string()],
        )
        .unwrap();
    drop(connection);

    let store = SqliteControlStore::open(&database).unwrap();
    let read = store
        .read(&[
            ControlFilter::all(ControlRecordKind::Project),
            ControlFilter::all(ControlRecordKind::Agent),
            ControlFilter::all(ControlRecordKind::Provider),
            ControlFilter::project(ControlRecordKind::Message, "p1"),
            ControlFilter::all(ControlRecordKind::Settings),
        ])
        .await
        .unwrap();
    assert_eq!(read.revision, 7);
    assert_eq!(read.records.len(), 6);
    let agents = read
        .records
        .iter()
        .filter(|record| record.kind == ControlRecordKind::Agent)
        .collect::<Vec<_>>();
    assert_eq!(agents.len(), 2);
    assert!(
        agents
            .iter()
            .all(|agent| agent.value.get("config").is_some())
    );
    assert!(agents.iter().all(|agent| agent.value.get("mode").is_none()));

    let mut updated = agents
        .iter()
        .find(|agent| agent.id == "a1")
        .expect("first migrated Agent")
        .value
        .clone();
    updated["name"] = serde_json::json!("updated");
    store
        .apply(
            read.revision,
            vec![ControlChange::Put(record(
                ControlRecordKind::Agent,
                "a1",
                None,
                updated,
            ))],
            Vec::new(),
        )
        .await
        .unwrap();
    drop(store);

    let reopened = SqliteControlStore::open(&database).unwrap();
    let agents = reopened
        .read(&[ControlFilter::all(ControlRecordKind::Agent)])
        .await
        .unwrap();
    assert_eq!(agents.records.len(), 2);
    assert!(
        agents
            .records
            .iter()
            .all(|agent| agent.value.get("config").is_some())
    );
    drop(reopened);

    let connection = Connection::open(database).unwrap();
    let legacy_table_count: u64 = connection
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='control_state'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(legacy_table_count, 0);
}

#[tokio::test]
async fn online_backup_restores_records_and_outbox() {
    let temporary = tempfile::TempDir::new().unwrap();
    let database = temporary.path().join("live.sqlite3");
    let backup = temporary.path().join("backup.sqlite3");
    let store = SqliteControlStore::open(&database).unwrap();
    store
        .apply(
            0,
            vec![ControlChange::Put(record(
                ControlRecordKind::Project,
                "p1",
                Some("p1"),
                serde_json::json!({"id":"p1","workdir":"/p1"}),
            ))],
            vec![PendingEvent {
                kind: "test.committed".into(),
                entity_id: Some("p1".into()),
                body: serde_json::json!({"revision":1}),
                created_at: 1,
            }],
        )
        .await
        .unwrap();
    store.backup_to(&backup).unwrap();
    store
        .apply(
            1,
            vec![ControlChange::Put(record(
                ControlRecordKind::Project,
                "p2",
                Some("p2"),
                serde_json::json!({"id":"p2","workdir":"/p2"}),
            ))],
            Vec::new(),
        )
        .await
        .unwrap();

    store.restore_from(&backup).unwrap();
    store.quick_check().unwrap();
    let recovered = store
        .read(&[ControlFilter::all(ControlRecordKind::Project)])
        .await
        .unwrap();
    assert_eq!(recovered.revision, 1);
    assert_eq!(recovered.records.len(), 1);
    assert_eq!(store.replay(0, 10).await.unwrap().len(), 1);
}

#[tokio::test]
async fn progress_reads_are_scoped_to_one_project() {
    let store = SqliteControlStore::in_memory().unwrap();
    for (run_id, project_id) in [("run-a", "project-a"), ("run-b", "project-b")] {
        store
            .save_progress(
                ProgressCheckpoint {
                    run_id: run_id.into(),
                    body: serde_json::json!({
                        "run_id": run_id,
                        "project_id": project_id,
                        "seq": 1
                    }),
                    updated_at: 1,
                },
                Vec::new(),
            )
            .await
            .unwrap();
    }

    let project_a = store.load_progress("project-a").await.unwrap();
    assert_eq!(project_a.len(), 1);
    assert_eq!(project_a[0].run_id, "run-a");
    assert_eq!(project_a[0].body["project_id"], "project-a");
    assert!(store.load_progress("missing").await.unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn replay_boundary_is_contiguous_or_resets_during_concurrent_retention() {
    let store = Arc::new(SqliteControlStore::in_memory().unwrap());
    let initial = (0..=RETAINED_EVENTS)
        .map(|index| PendingEvent {
            kind: if index == 1 {
                "run.updated".into()
            } else {
                "run.progress".into()
            },
            entity_id: Some("run-a".into()),
            body: serde_json::json!({"index":index}),
            created_at: 1,
        })
        .collect();
    store
        .save_progress(
            ProgressCheckpoint {
                run_id: "run-a".into(),
                body: serde_json::json!({"seq":RETAINED_EVENTS}),
                updated_at: 1,
            },
            initial,
        )
        .await
        .unwrap();
    assert_eq!(store.event_bounds().await.unwrap().oldest, Some(2));

    let barrier = Arc::new(tokio::sync::Barrier::new(2));
    let reader_store = store.clone();
    let reader_barrier = barrier.clone();
    let reader = tokio::spawn(async move {
        reader_barrier.wait().await;
        reader_store.replay_page(1, 8).await.unwrap()
    });
    let writer = tokio::spawn(async move {
        barrier.wait().await;
        store
            .save_progress(
                ProgressCheckpoint {
                    run_id: "run-a".into(),
                    body: serde_json::json!({"seq":RETAINED_EVENTS + 64}),
                    updated_at: 2,
                },
                (0..64)
                    .map(|index| PendingEvent {
                        kind: "run.progress".into(),
                        entity_id: Some("run-a".into()),
                        body: serde_json::json!({"new":index}),
                        created_at: 2,
                    })
                    .collect(),
            )
            .await
            .unwrap();
    });
    let (page, writer) = tokio::join!(reader, writer);
    writer.unwrap();
    let page = page.unwrap();
    if page.cursor_valid {
        assert_eq!(page.events.first().unwrap().cursor, 2);
    } else {
        assert!(page.events.is_empty());
        assert!(page.bounds.oldest.is_some_and(|oldest| oldest > 2));
    }
}
