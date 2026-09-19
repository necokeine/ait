use super::*;

#[tokio::test]
async fn interrupted_conversion_keeps_format_barrier_and_resumes_from_local_receipt() {
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
    let catalog = temp.path().join("catalog");
    let id = format!("conversion-{}", temp.path().display());
    let legacy = crate::SplitSqliteControlStore::open(&catalog).unwrap();
    legacy.apply(0,vec![put(Kind::Project,&id,Some(&id),json!({"id":id,"workdir":root,"root_message_id":"root","name":"Original"})),put(Kind::Message,"root",Some(&id),json!({"id":"root","project_id":id,"parent_message_id":null,"text":"preserved"}))],vec![]).await.unwrap();
    drop(legacy);
    let local = Connection::open(root.join(".ait/project.sqlite3")).unwrap();
    // Fail the first local DDL after the global format barrier has committed.
    local
        .execute_batch("CREATE TABLE configuration_sources(injected INTEGER)")
        .unwrap();
    assert!(PortableSqliteControlStore::upgrade_storage(&catalog).is_err());
    assert!(PortableSqliteControlStore::open(&catalog).is_err());
    assert!(crate::SplitSqliteControlStore::open(&catalog).is_err());
    assert_eq!(
        local
            .pragma_query_value(None, "user_version", |r| r.get::<_, u32>(0))
            .unwrap(),
        2
    );
    local
        .execute_batch("DROP TABLE configuration_sources")
        .unwrap();
    drop(local);
    PortableSqliteControlStore::upgrade_storage(&catalog).unwrap();
    let store = PortableSqliteControlStore::open(&catalog).unwrap();
    store.open_project(root.to_str().unwrap()).await.unwrap();
    let version = store
        .read(&[ControlFilter::id(Kind::Project, &id)])
        .await
        .unwrap()
        .version;
    store
        .apply_versioned(
            &version,
            vec![put(
                Kind::Message,
                "after",
                Some(&id),
                json!({"id":"after","project_id":id,"parent_message_id":"root","text":"new"}),
            )],
            vec![],
        )
        .await
        .unwrap();
    drop(store);
    // Simulate lost catalog acknowledgement after a completed local conversion.
    let global = Connection::open(&catalog).unwrap();
    global
        .execute("UPDATE conversion_manifest SET phase='preparing'", [])
        .unwrap();
    drop(global);
    PortableSqliteControlStore::upgrade_storage(&catalog).unwrap();
    let store = PortableSqliteControlStore::open(&catalog).unwrap();
    store.open_project(root.to_str().unwrap()).await.unwrap();
    let history = store
        .read(&[ControlFilter::message_ancestors("after")])
        .await
        .unwrap();
    assert_eq!(history.records.len(), 2);
    assert!(
        history
            .records
            .iter()
            .any(|record| record.value["text"] == "preserved")
    );
}

#[tokio::test]
async fn draining_rejects_new_runs_and_workers_but_allows_existing_run_settlement() {
    let temp = TempDir::new().unwrap();
    let id = format!("drain-{}", temp.path().display());
    let store = PortableSqliteControlStore::open(temp.path().join("catalog")).unwrap();
    create(&store, &temp.path().join("project"), &id).await;
    let read = store
        .read(&[ControlFilter::id(Kind::Project, &id)])
        .await
        .unwrap();
    store
        .apply_versioned(
            &read.version,
            vec![put(
                Kind::Run,
                "existing",
                Some(&id),
                json!({"id":"existing","project_id":id,"status":"running"}),
            )],
            vec![],
        )
        .await
        .unwrap();
    store.begin_project_drain(&id).await.unwrap();
    let read = store
        .read(&[ControlFilter::id(Kind::Project, &id)])
        .await
        .unwrap();
    assert!(
        store
            .register_worker_process(&read.version.projects[&id].owner(&id), 12345)
            .await
            .unwrap_err()
            .to_string()
            .contains("PROJECT_DRAINING")
    );
    assert!(
        store
            .apply_versioned(
                &read.version,
                vec![put(
                    Kind::Run,
                    "new",
                    Some(&id),
                    json!({"id":"new","project_id":id})
                )],
                vec![]
            )
            .await
            .unwrap_err()
            .to_string()
            .contains("PROJECT_DRAINING")
    );
    store
        .apply_versioned(
            &read.version,
            vec![put(
                Kind::Run,
                "existing",
                Some(&id),
                json!({"id":"existing","project_id":id,"status":"cancelled"}),
            )],
            vec![],
        )
        .await
        .unwrap();
    store.close_project(&id).await.unwrap();
    assert!(!store.project_is_open(&id).await.unwrap());
}
