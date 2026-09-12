use crate::SqliteControlStore;
use ait_ports::ControlRecord;
use serde_json::json;
use tempfile::TempDir;

use super::*;

#[tokio::test]
async fn legacy_blob_migrates_directly_to_project_files() {
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("a");
    git_project(&root);
    let database = temp.path().join("legacy.sqlite3");
    let connection = Connection::open(&database).unwrap();
    connection.execute_batch("CREATE TABLE control_state(singleton INTEGER PRIMARY KEY,revision INTEGER NOT NULL,body_json TEXT NOT NULL);").unwrap();
    connection
        .execute(
            "INSERT INTO control_state VALUES(1,7,?1)",
            [json!({
                "projects":[{"id":"a","workdir":root}], "agents":[], "sessions":[],
                "messages":[{"id":"root","project_id":"a","parent_message_id":null}],
                "runs":[], "crons":[], "settings":{}, "settings_revision":3
            })
            .to_string()],
        )
        .unwrap();
    drop(connection);
    let store = SplitSqliteControlStore::open(&database).unwrap();
    let read = store
        .read(&[ControlFilter::message_ancestors("root")])
        .await
        .unwrap();
    assert_eq!(read.revision, 7);
    assert_eq!(read.records.len(), 1);
    let global = Connection::open(database).unwrap();
    assert_eq!(
        count(
            &global,
            "SELECT count(*) FROM sqlite_master WHERE name IN ('control_state','messages')"
        ),
        0
    );
}

#[tokio::test]
async fn another_catalog_cannot_take_over_existing_history() {
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("a");
    git_project(&root);
    let first = SplitSqliteControlStore::open(temp.path().join("first.sqlite3")).unwrap();
    first
        .apply(0, project_records(&root, "a"), Vec::new())
        .await
        .unwrap();
    let second = SplitSqliteControlStore::open(temp.path().join("second.sqlite3")).unwrap();
    assert!(
        second
            .apply(0, project_records(&root, "a"), Vec::new())
            .await
            .unwrap_err()
            .to_string()
            .contains("identity/coordinator")
    );
    assert!(
        second
            .read(&[ControlFilter::all(ControlRecordKind::Project)])
            .await
            .unwrap()
            .records
            .is_empty()
    );
    assert_eq!(
        first
            .read(&[ControlFilter::message_ancestors("a-root")])
            .await
            .unwrap()
            .records
            .len(),
        1
    );
}

#[cfg(unix)]
#[tokio::test]
async fn project_storage_symlinks_cannot_redirect_creation() {
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("a");
    git_project(&root);
    let outside = temp.path().join("outside");
    std::fs::create_dir(&outside).unwrap();
    std::os::unix::fs::symlink(&outside, root.join(".metafab")).unwrap();
    let store = SplitSqliteControlStore::open(temp.path().join("global.sqlite3")).unwrap();
    assert!(
        store
            .apply(0, project_records(&root, "a"), Vec::new())
            .await
            .unwrap_err()
            .to_string()
            .contains("symlink")
    );
    assert!(std::fs::read_dir(outside).unwrap().next().is_none());
    assert_eq!(store.read(&[]).await.unwrap().revision, 0);
}

#[tokio::test]
async fn split_event_retention_preserves_cursor_reset_and_project_payloads() {
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("a");
    git_project(&root);
    let store = SplitSqliteControlStore::open(temp.path().join("global.sqlite3")).unwrap();
    store
        .apply(0, project_records(&root, "a"), Vec::new())
        .await
        .unwrap();
    store
        .save_progress(
            checkpoint("a"),
            (0..RETAINED_EVENTS + 2).map(|_| event("a")).collect(),
        )
        .await
        .unwrap();
    assert_eq!(store.event_bounds().await.unwrap().oldest, Some(3));
    assert!(!store.replay_page(1, 10).await.unwrap().cursor_valid);
    let page = store.replay_page(2, 256).await.unwrap();
    assert!(page.cursor_valid);
    assert_eq!(page.events.len(), 256);
    assert_eq!(page.events[0].cursor, 3);
    assert_eq!(page.events[0].body["text"], "private event a");
}

fn git_project(root: &Path) {
    std::fs::create_dir(root).unwrap();
    assert!(
        std::process::Command::new("git")
            .arg("-C")
            .arg(root)
            .args(["init", "--quiet"])
            .status()
            .unwrap()
            .success()
    );
}

fn put(kind: ControlRecordKind, id: &str, project_id: Option<&str>, value: Value) -> ControlChange {
    ControlChange::Put(ControlRecord {
        kind,
        id: id.into(),
        project_id: project_id.map(str::to_owned),
        value,
    })
}

fn project_records(root: &Path, id: &str) -> Vec<ControlChange> {
    vec![
        put(
            ControlRecordKind::Project,
            id,
            Some(id),
            json!({"id":id,"workdir":root}),
        ),
        put(
            ControlRecordKind::Message,
            &format!("{id}-root"),
            Some(id),
            json!({"id":format!("{id}-root"),"project_id":id,"parent_message_id":null,"text":format!("private history {id}")}),
        ),
        put(
            ControlRecordKind::Session,
            &format!("{id}-session"),
            Some(id),
            json!({"id":format!("{id}-session"),"project_id":id,"current_message_id":format!("{id}-root"),"version":0}),
        ),
        put(
            ControlRecordKind::Run,
            &format!("{id}-run"),
            Some(id),
            json!({"id":format!("{id}-run"),"project_id":id,"session_id":format!("{id}-session"),"cron_id":"cron","status":"queued","config":{"model":"snapshot-model"}}),
        ),
        put(
            ControlRecordKind::RunCredential,
            &format!("{id}-run"),
            Some(id),
            json!("credential-reference"),
        ),
        put(
            ControlRecordKind::WorkspaceRunJournal,
            &format!("{id}-run"),
            Some(id),
            json!({"phase":"prepared"}),
        ),
    ]
}

fn event(id: &str) -> PendingEvent {
    PendingEvent {
        kind: "run.updated".into(),
        entity_id: Some(format!("{id}-run")),
        body: json!({"project_id":id,"text":format!("private event {id}")}),
        created_at: 1,
    }
}

fn checkpoint(id: &str) -> ProgressCheckpoint {
    ProgressCheckpoint {
        run_id: format!("{id}-run"),
        body: json!({"project_id":id,"text":"private progress"}),
        updated_at: 1,
    }
}

fn count(connection: &Connection, sql: &str) -> u64 {
    connection.query_row(sql, [], |row| row.get(0)).unwrap()
}

#[tokio::test]
#[allow(
    clippy::too_many_lines,
    reason = "verifies the complete physical boundary and standalone backups"
)]
async fn histories_events_progress_and_backups_are_physically_separate() {
    let temp = TempDir::new().unwrap();
    let database = temp.path().join("global.sqlite3");
    let store = SplitSqliteControlStore::open(&database).unwrap();
    let mut changes = Vec::new();
    for id in ["a", "b"] {
        let root = temp.path().join(id);
        git_project(&root);
        changes.extend(project_records(&root, id));
    }
    changes.push(put(
        ControlRecordKind::Agent,
        "agent",
        None,
        json!({"id":"agent"}),
    ));
    changes.push(put(
        ControlRecordKind::Cron,
        "cron",
        Some("a"),
        json!({"id":"cron","project_id":"a"}),
    ));
    store
        .apply(0, changes, vec![event("a"), event("b")])
        .await
        .unwrap();
    store
        .save_progress(checkpoint("a"), vec![event("a")])
        .await
        .unwrap();
    assert_eq!(store.load_progress("a").await.unwrap().len(), 1);
    assert!(store.load_progress("b").await.unwrap().is_empty());
    assert_eq!(store.replay(0, 10).await.unwrap().len(), 3);
    let read = store
        .read(&[
            ControlFilter::message_ancestors("a-root"),
            ControlFilter::runs_for_session("a-session"),
            ControlFilter::runs_for_cron("cron"),
        ])
        .await
        .unwrap();
    assert_eq!(read.records.len(), 2);
    assert!(
        read.records
            .iter()
            .all(|record| record.project_id.as_deref() == Some("a"))
    );
    let global = Connection::open(&database).unwrap();
    assert_eq!(
        count(
            &global,
            "SELECT count(*) FROM sqlite_master WHERE name IN ('messages','sessions','runs','workspace_run_journals','run_credentials','run_progress')"
        ),
        0
    );
    assert_eq!(
        count(
            &global,
            "SELECT count(*) FROM durable_events WHERE body_json != 'null'"
        ),
        0
    );
    assert_eq!(count(&global, "SELECT count(*) FROM pending_commit"), 0);
    for id in ["a", "b"] {
        let root = temp.path().join(id);
        let project = Connection::open(root.join(".metafab/project.sqlite3")).unwrap();
        assert_eq!(count(&project, "SELECT count(*) FROM messages"), 1);
        assert_eq!(
            count(
                &project,
                "SELECT count(*) FROM sqlite_master WHERE name IN ('projects','agents','agent_providers','provider_credentials','settings','crons')"
            ),
            0
        );
        check_foreign_keys(&project).unwrap();
        let status = std::process::Command::new("git")
            .arg("-C")
            .arg(&root)
            .args(["status", "--porcelain"])
            .output()
            .unwrap();
        assert!(status.status.success());
        assert!(status.stdout.is_empty());
    }
    let global_backup = temp.path().join("global-backup.sqlite3");
    let project_backup = temp.path().join("project-backup.sqlite3");
    store.backup_global_to(&global_backup).unwrap();
    store.backup_project_to("a", &project_backup).unwrap();
    assert_eq!(
        count(
            &Connection::open(global_backup).unwrap(),
            "SELECT count(*) FROM projects"
        ),
        2
    );
    let backup = Connection::open(project_backup).unwrap();
    assert_eq!(count(&backup, "SELECT count(*) FROM messages"), 1);
    assert_eq!(
        backup
            .query_row("PRAGMA integrity_check", [], |row| row.get::<_, String>(0))
            .unwrap(),
        "ok"
    );
    store.clear_progress("a-run").await.unwrap();
    assert!(store.load_progress("a").await.unwrap().is_empty());
    drop(store);
    let reopened = SplitSqliteControlStore::open(database).unwrap();
    assert_eq!(
        reopened
            .read(&[ControlFilter::all(ControlRecordKind::Message)])
            .await
            .unwrap()
            .records
            .len(),
        2
    );
}

#[tokio::test]
async fn rejected_batches_leave_no_visible_partial_changes() {
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("a");
    git_project(&root);
    let store = SplitSqliteControlStore::open(temp.path().join("global.sqlite3")).unwrap();
    store
        .apply(0, project_records(&root, "a"), Vec::new())
        .await
        .unwrap();
    for change in [
        put(
            ControlRecordKind::Message,
            "a-root",
            Some("a"),
            json!({"project_id":"a","text":"changed"}),
        ),
        put(
            ControlRecordKind::Message,
            "bad",
            Some("a"),
            json!({"project_id":"a","parent_message_id":"other-project-message"}),
        ),
        put(
            ControlRecordKind::Run,
            "a-run",
            Some("b"),
            json!({"project_id":"b"}),
        ),
    ] {
        assert!(
            store
                .apply(
                    1,
                    vec![
                        put(ControlRecordKind::Agent, "new", None, json!({})),
                        change
                    ],
                    Vec::new()
                )
                .await
                .is_err()
        );
        let read = store
            .read(&[ControlFilter::all(ControlRecordKind::Agent)])
            .await
            .unwrap();
        assert_eq!(read.revision, 1);
        assert!(read.records.is_empty());
    }
}

#[tokio::test]
async fn unavailable_projects_do_not_break_catalog_or_other_project_reads() {
    let temp = TempDir::new().unwrap();
    let database = temp.path().join("global.sqlite3");
    let store = SplitSqliteControlStore::open(&database).unwrap();
    let mut changes = Vec::new();
    for id in ["a", "b"] {
        let root = temp.path().join(id);
        git_project(&root);
        changes.extend(project_records(&root, id));
    }
    store.apply(0, changes, Vec::new()).await.unwrap();
    drop(store);
    std::fs::rename(temp.path().join("b"), temp.path().join("offline-b")).unwrap();
    let store = SplitSqliteControlStore::open(&database).unwrap();
    assert_eq!(
        store
            .read(&[ControlFilter::all(ControlRecordKind::Project)])
            .await
            .unwrap()
            .records
            .len(),
        2
    );
    assert_eq!(
        store
            .read(&[ControlFilter::project(ControlRecordKind::Message, "a")])
            .await
            .unwrap()
            .records
            .len(),
        1
    );
    assert!(
        store
            .read(&[ControlFilter::project(ControlRecordKind::Message, "b")])
            .await
            .is_err()
    );
    assert!(!temp.path().join("b").exists());
    store
        .apply(
            1,
            vec![
                put(ControlRecordKind::Agent, "agent", None, json!({})),
                put(
                    ControlRecordKind::Project,
                    "b",
                    Some("b"),
                    json!({"id":"b","workdir":temp.path().join("b"),"name":"Offline project"}),
                ),
            ],
            Vec::new(),
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn migration_preserves_revision_cursors_and_all_project_records() {
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("a");
    git_project(&root);
    let database = temp.path().join("legacy.sqlite3");
    let legacy = SqliteControlStore::open(&database).unwrap();
    legacy
        .apply(0, project_records(&root, "a"), vec![event("a")])
        .await
        .unwrap();
    legacy
        .save_progress(checkpoint("a"), vec![event("a")])
        .await
        .unwrap();
    drop(legacy);
    let store = SplitSqliteControlStore::open(&database).unwrap();
    let read = store
        .read(&PROJECT_KINDS.map(ControlFilter::all))
        .await
        .unwrap();
    assert_eq!(read.revision, 1);
    assert_eq!(read.records.len(), 5);
    assert_eq!(store.replay(1, 10).await.unwrap()[0].cursor, 2);
    assert_eq!(store.load_progress("a").await.unwrap().len(), 1);
    assert!(
        temp.path()
            .join("legacy.sqlite3.pre-split.sqlite3")
            .exists()
    );
    drop(store);
    let reopened = SplitSqliteControlStore::open(&database).unwrap();
    reopened
        .apply(1, Vec::new(), vec![event("a")])
        .await
        .unwrap();
    assert_eq!(reopened.replay(2, 10).await.unwrap()[0].cursor, 3);
}

#[tokio::test]
async fn failed_migration_preserves_source_and_retries_after_project_returns() {
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("a");
    git_project(&root);
    let database = temp.path().join("legacy.sqlite3");
    let legacy = SqliteControlStore::open(&database).unwrap();
    legacy
        .apply(0, project_records(&root, "a"), Vec::new())
        .await
        .unwrap();
    drop(legacy);
    std::fs::rename(&root, temp.path().join("offline")).unwrap();
    assert!(SplitSqliteControlStore::open(&database).is_err());
    assert_eq!(
        count(
            &Connection::open(&database).unwrap(),
            "SELECT count(*) FROM messages"
        ),
        1
    );
    std::fs::rename(temp.path().join("offline"), &root).unwrap();
    let store = SplitSqliteControlStore::open(&database).unwrap();
    assert_eq!(
        store
            .read(&[ControlFilter::all(ControlRecordKind::Message)])
            .await
            .unwrap()
            .records
            .len(),
        1
    );
}

#[tokio::test]
async fn prepared_and_decided_crash_windows_recover_exactly_once() {
    // 0: prepared only, 1: decision, 2: first Project applied, 3: all Projects applied.
    for boundary in 0_usize..4 {
        let temp = TempDir::new().unwrap();
        let database = temp.path().join("global.sqlite3");
        let store = SplitSqliteControlStore::open(&database).unwrap();
        let mut changes = Vec::new();
        for id in ["a", "b"] {
            let root = temp.path().join(id);
            git_project(&root);
            changes.extend(project_records(&root, id));
        }
        store.apply(0, changes, Vec::new()).await.unwrap();
        {
            let connection = store.connection.lock().unwrap();
            let mut changes = vec![put(
                ControlRecordKind::Agent,
                "agent",
                None,
                json!({"id":"agent"}),
            )];
            for id in ["a", "b"] {
                changes.push(put(
                    ControlRecordKind::Message,
                    &format!("{id}-child"),
                    Some(id),
                    json!({"project_id":id,"parent_message_id":format!("{id}-root")}),
                ));
            }
            let (mut commit, mut batches) = build_commit(&connection, 2, changes).unwrap();
            route_events(
                &connection,
                &mut commit,
                &mut batches,
                vec![event("a"), event("b")],
            )
            .unwrap();
            let owner = owner(&connection).unwrap();
            for (id, batch) in &batches {
                prepare_project(
                    &mut open_project(&commit.targets[id], &owner, false).unwrap(),
                    &commit.id,
                    batch,
                )
                .unwrap();
            }
            if boundary > 0 {
                connection
                    .execute(
                        "INSERT INTO pending_commit VALUES(1,?1)",
                        [serde_json::to_string(&commit).unwrap()],
                    )
                    .unwrap();
            }
            for target in commit.targets.values().take(boundary.saturating_sub(1)) {
                finish_project(
                    &mut open_project(target, &owner, false).unwrap(),
                    &commit.id,
                )
                .unwrap();
            }
        }
        drop(store);
        for _ in 0..2 {
            let store = SplitSqliteControlStore::open(&database).unwrap();
            let read = store
                .read(&[ControlFilter::all(ControlRecordKind::Message)])
                .await
                .unwrap();
            assert_eq!(read.revision, if boundary == 0 { 1 } else { 2 });
            assert_eq!(read.records.len(), if boundary == 0 { 2 } else { 4 });
            assert_eq!(
                store.replay(0, 10).await.unwrap().len(),
                if boundary == 0 { 0 } else { 2 }
            );
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn independent_connections_share_the_revision_cas() {
    let temp = TempDir::new().unwrap();
    let database = temp.path().join("global.sqlite3");
    let first = SplitSqliteControlStore::open(&database).unwrap();
    let second = SplitSqliteControlStore::open(&database).unwrap();
    let a = tokio::spawn(async move {
        first
            .apply(
                0,
                vec![put(ControlRecordKind::Agent, "a", None, json!({}))],
                Vec::new(),
            )
            .await
    });
    let b = tokio::spawn(async move {
        second
            .apply(
                0,
                vec![put(ControlRecordKind::Agent, "b", None, json!({}))],
                Vec::new(),
            )
            .await
    });
    let results = [a.await.unwrap(), b.await.unwrap()];
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter(|result| **result == Err(ControlStoreError::Conflict))
            .count(),
        1
    );
}

#[tokio::test]
async fn foreign_identity_and_future_versions_are_rejected_without_mutation() {
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("a");
    git_project(&root);
    let database = temp.path().join("global.sqlite3");
    let store = SplitSqliteControlStore::open(&database).unwrap();
    store
        .apply(0, project_records(&root, "a"), Vec::new())
        .await
        .unwrap();
    let project = Connection::open(root.join(".metafab/project.sqlite3")).unwrap();
    project.pragma_update(None, "foreign_keys", false).unwrap();
    project
        .execute("UPDATE project_identity SET project_id='other'", [])
        .unwrap();
    assert!(
        store
            .read(&[ControlFilter::project(ControlRecordKind::Message, "a")])
            .await
            .unwrap_err()
            .to_string()
            .contains("identity")
    );
    project
        .execute("UPDATE project_identity SET project_id='a'", [])
        .unwrap();
    project.pragma_update(None, "user_version", 99).unwrap();
    assert!(
        store
            .read(&[ControlFilter::project(ControlRecordKind::Message, "a")])
            .await
            .unwrap_err()
            .to_string()
            .contains("format")
    );
    assert_eq!(pragma_number(&project, "user_version").unwrap(), 99);
    drop(store);
    let global = Connection::open(&database).unwrap();
    global.pragma_update(None, "user_version", 99).unwrap();
    assert!(SplitSqliteControlStore::open(&database).is_err());
}
