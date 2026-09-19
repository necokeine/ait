use super::*;
use serde_json::json;
use tempfile::TempDir;

fn put(kind: Kind, id: &str, project_id: Option<&str>, value: Value) -> ControlChange {
    ControlChange::Put(ControlRecord {
        kind,
        id: id.into(),
        project_id: project_id.map(str::to_owned),
        value,
    })
}

async fn create(store: &PortableSqliteControlStore, root: &Path, id: &str) {
    std::fs::create_dir_all(root).unwrap();
    assert!(
        std::process::Command::new("git")
            .args(["init", "-q"])
            .arg(root)
            .status()
            .unwrap()
            .success()
    );
    let version = store.read(&[]).await.unwrap().version;
    store
        .apply_versioned(
            &version,
            vec![
                put(
                    Kind::Project,
                    id,
                    Some(id),
                    json!({"id":id,"workdir":root,"name":"Project","root_message_id":"root"}),
                ),
                put(
                    Kind::Message,
                    "root",
                    Some(id),
                    json!({"id":"root","project_id":id,"parent_message_id":null,"role":"system"}),
                ),
            ],
            vec![PendingEvent {
                kind: "project.registered".into(),
                entity_id: Some(id.into()),
                body: json!({"project_id":id}),
                created_at: 1,
            }],
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn another_catalog_opens_the_same_project_after_runtime_release() {
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("project");
    let id = format!("project-{}", temp.path().display());
    let first = PortableSqliteControlStore::open(temp.path().join("first.sqlite3")).unwrap();
    create(&first, &root, &id).await;
    let second = PortableSqliteControlStore::open(temp.path().join("second.sqlite3")).unwrap();
    assert!(
        second
            .open_project(root.to_str().unwrap())
            .await
            .unwrap_err()
            .to_string()
            .contains("PROJECT_BUSY")
    );
    let before = first
        .read(&[ControlFilter::message_ancestors("root")])
        .await
        .unwrap();
    first.close_project(&id).await.unwrap();
    let reopened = second
        .open_project(root.to_str().unwrap())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(reopened.id, id);
    let after = second
        .read(&[ControlFilter::message_ancestors("root")])
        .await
        .unwrap();
    assert_eq!(before.records, after.records);
    assert!(after.version.projects[&id].owner_epoch > before.version.projects[&id].owner_epoch);
    assert!(
        first
            .apply_versioned(&before.version, vec![], vec![])
            .await
            .is_err()
    );
}

#[tokio::test]
async fn project_revision_is_local_and_catalog_indexes_are_rebuildable() {
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("project");
    let id = format!("revision-{}", temp.path().display());
    let store = PortableSqliteControlStore::open(temp.path().join("catalog.sqlite3")).unwrap();
    create(&store, &root, &id).await;
    let before = store
        .read(&[ControlFilter::id(Kind::Project, &id)])
        .await
        .unwrap();
    store
        .apply_versioned(
            &before.version,
            vec![put(
                Kind::Message,
                "answer",
                Some(&id),
                json!({"id":"answer","project_id":id,"parent_message_id":"root","text":"saved"}),
            )],
            vec![],
        )
        .await
        .unwrap();
    let after = store
        .read(&[ControlFilter::id(Kind::Project, &id)])
        .await
        .unwrap();
    assert_eq!(
        before.version.catalog_revision,
        after.version.catalog_revision
    );
    assert_eq!(
        after.version.projects[&id].revision,
        before.version.projects[&id].revision + 1
    );
    assert!(
        store
            .apply_versioned(&before.version, vec![], vec![])
            .await
            .is_err()
    );
    drop(store);
    std::fs::remove_file(temp.path().join("catalog.sqlite3")).unwrap();
    let fresh = PortableSqliteControlStore::open(temp.path().join("catalog.sqlite3")).unwrap();
    fresh.open_project(root.to_str().unwrap()).await.unwrap();
    assert_eq!(
        fresh
            .read(&[ControlFilter::message_ancestors("answer")])
            .await
            .unwrap()
            .records
            .len(),
        2
    );
}

#[tokio::test]
async fn conversion_preserves_legacy_history_and_can_be_repeated() {
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("project");
    std::fs::create_dir(&root).unwrap();
    assert!(
        std::process::Command::new("git")
            .args(["init", "-q"])
            .arg(&root)
            .status()
            .unwrap()
            .success()
    );
    let catalog = temp.path().join("catalog.sqlite3");
    let old = crate::SplitSqliteControlStore::open(&catalog).unwrap();
    let id = format!("legacy-{}", temp.path().display());
    old.apply(0,vec![put(Kind::Project,&id,Some(&id),json!({"id":id,"workdir":root,"root_message_id":"root","name":"Saved"})),put(Kind::Message,"root",Some(&id),json!({"id":"root","project_id":id,"parent_message_id":null,"role":"system","text":"Keep me"}))],vec![]).await.unwrap();
    let original = old
        .read(&[ControlFilter::message_ancestors("root")])
        .await
        .unwrap()
        .records;
    drop(old);
    assert!(PortableSqliteControlStore::open(&catalog).is_err());
    PortableSqliteControlStore::upgrade_storage(&catalog).unwrap();
    PortableSqliteControlStore::upgrade_storage(&catalog).unwrap();
    assert!(crate::SplitSqliteControlStore::open(&catalog).is_err());
    let current = PortableSqliteControlStore::open(&catalog).unwrap();
    current.open_project(root.to_str().unwrap()).await.unwrap();
    assert_eq!(
        current
            .read(&[ControlFilter::message_ancestors("root")])
            .await
            .unwrap()
            .records,
        original
    );
}

mod configuration;
mod conversion;
mod processes;
mod transactions;
