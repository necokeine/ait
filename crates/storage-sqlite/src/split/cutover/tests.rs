//! The format cutover drops conversation records once and never touches work files.
use super::super::*;
use ait_ports::ControlRecord;
use serde_json::json;
fn put(kind: ControlRecordKind, id: &str, value: Value) -> ControlChange {
    ControlChange::Put(ControlRecord {
        kind,
        id: id.into(),
        project_id: Some("p".into()),
        value,
    })
}
async fn fixture() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("project");
    std::fs::create_dir(&root).unwrap();
    assert!(
        std::process::Command::new("git")
            .arg("-C")
            .arg(&root)
            .args(["init", "-q"])
            .status()
            .unwrap()
            .success()
    );
    let database = directory.path().join("global.sqlite3");
    let store = SplitSqliteControlStore::open(&database).unwrap();
    store.apply(0,vec![
        put(ControlRecordKind::Project,"p",json!({"id":"p","workdir":root,"root_message_id":"root"})),
        put(ControlRecordKind::Message,"root",json!({"id":"root","project_id":"p","parent_message_id":null,"text":"Project instructions"})),
        put(ControlRecordKind::Message,"old-message",json!({"id":"old-message","project_id":"p","parent_message_id":"root","text":"old session"})),
        put(ControlRecordKind::Session,"old-session",json!({"id":"old-session","project_id":"p"})),
        put(ControlRecordKind::Run,"old-run",json!({"id":"old-run","project_id":"p"})),
        put(ControlRecordKind::Agent,"owned",json!({"id":"owned","owner_session_id":"old-session"})),
        put(ControlRecordKind::Agent,"named",json!({"id":"named","owner_session_id":null})),
        put(ControlRecordKind::Cron,"old-cron",json!({"id":"old-cron","project_id":"p"})),
    ],vec![]).await.unwrap();
    drop(store);
    // Simulate a catalog created before the new execution format was introduced.
    Connection::open(&database)
        .unwrap()
        .execute_batch("DROP TABLE native_execution_cutover; PRAGMA user_version=1;")
        .unwrap();
    std::fs::create_dir(root.join(".ait/old-session")).unwrap();
    std::fs::write(root.join(".ait/old-session/user-work.txt"), "keep").unwrap();
    std::fs::write(
        directory.path().join("native-rollout.jsonl"),
        "external Codex history",
    )
    .unwrap();
    (directory, root, database)
}
#[tokio::test]
async fn clear_once_preserves_root_catalog_files_and_new_sessions() {
    let (directory, root, database) = fixture().await;
    let store = SplitSqliteControlStore::open(&database).unwrap();
    let records = store
        .read(&[
            ControlFilter::project(ControlRecordKind::Message, "p"),
            ControlFilter::project(ControlRecordKind::Session, "p"),
            ControlFilter::project(ControlRecordKind::Run, "p"),
        ])
        .await
        .unwrap();
    assert_eq!(records.records.len(), 1);
    assert_eq!(records.records[0].id, "root");
    let agents = store
        .read(&[ControlFilter::all(ControlRecordKind::Agent)])
        .await
        .unwrap();
    assert_eq!(agents.records.len(), 1);
    assert_eq!(agents.records[0].id, "named");
    assert!(
        store
            .read(&[ControlFilter::all(ControlRecordKind::Cron)])
            .await
            .unwrap()
            .records
            .is_empty()
    );
    assert_eq!(
        std::fs::read_to_string(root.join(".ait/old-session/user-work.txt")).unwrap(),
        "keep"
    );
    assert!(directory.path().join("native-rollout.jsonl").exists());
    store
        .apply(
            records.revision,
            vec![put(
                ControlRecordKind::Session,
                "new",
                json!({"id":"new","project_id":"p"}),
            )],
            vec![],
        )
        .await
        .unwrap();
    drop(store);
    let reopened = SplitSqliteControlStore::open(&database).unwrap();
    assert_eq!(
        reopened
            .read(&[ControlFilter::project(ControlRecordKind::Session, "p")])
            .await
            .unwrap()
            .records[0]
            .id,
        "new"
    );
}
#[tokio::test]
async fn offline_project_is_reset_on_verified_reopen() {
    let (directory, root, database) = fixture().await;
    let offline = directory.path().join("offline");
    std::fs::rename(&root, &offline).unwrap();
    let store = SplitSqliteControlStore::open(&database).unwrap();
    assert!(
        store
            .read(&[ControlFilter::project(ControlRecordKind::Session, "p")])
            .await
            .is_err()
    );
    std::fs::rename(offline, root).unwrap();
    assert!(
        store
            .read(&[ControlFilter::project(ControlRecordKind::Session, "p")])
            .await
            .unwrap()
            .records
            .is_empty()
    );
}
#[tokio::test]
async fn wrong_coordinator_is_rejected_before_destructive_cutover() {
    let (_directory, root, database) = fixture().await;
    let project = Connection::open(root.join(".ait/project.sqlite3")).unwrap();
    project
        .execute("UPDATE project_identity SET coordinator_id='other'", [])
        .unwrap();
    let store = SplitSqliteControlStore::open(&database).unwrap();
    assert!(
        store
            .read(&[ControlFilter::project(ControlRecordKind::Session, "p")])
            .await
            .is_err()
    );
    let count: u64 = project
        .query_row("SELECT count(*) FROM sessions", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 1);
}
