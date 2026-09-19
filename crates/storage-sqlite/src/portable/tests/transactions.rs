use super::*;

#[tokio::test]
async fn lost_response_replays_receipt_but_modified_request_conflicts() {
    let temp = TempDir::new().unwrap();
    let id = format!("receipt-{}", temp.path().display());
    let store = PortableSqliteControlStore::open(temp.path().join("catalog")).unwrap();
    create(&store, &temp.path().join("project"), &id).await;
    let version = store
        .read(&[ControlFilter::id(Kind::Project, &id)])
        .await
        .unwrap()
        .version;
    let change = put(
        Kind::Message,
        "answer",
        Some(&id),
        json!({"id":"answer","project_id":id,"parent_message_id":"root","text":"once"}),
    );
    let events = vec![PendingEvent {
        kind: "answer".into(),
        entity_id: None,
        body: json!({"private":"content"}),
        created_at: 2,
    }];
    let committed = store
        .apply_versioned(&version, vec![change.clone()], events.clone())
        .await
        .unwrap();
    assert_eq!(
        store
            .apply_versioned(&version, vec![change], events)
            .await
            .unwrap(),
        committed
    );
    assert!(matches!(
        store.apply_versioned(&version, vec![], vec![]).await,
        Err(ControlStoreError::Conflict)
    ));
    assert_eq!(store.replay(0, 100).await.unwrap().len(), 2);
    store
        .access(|state| {
            let body: String = state
                .catalog
                .query_row(
                    "SELECT body_json FROM durable_events WHERE kind='answer'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(body, "{}");
            Ok(())
        })
        .unwrap();
}

#[tokio::test]
async fn project_only_commit_ignores_unrelated_catalog_revision_but_configuration_does_not() {
    let temp = TempDir::new().unwrap();
    let id = format!("cas-{}", temp.path().display());
    let store = PortableSqliteControlStore::open(temp.path().join("catalog")).unwrap();
    create(&store, &temp.path().join("project"), &id).await;
    let project = store
        .read(&[ControlFilter::id(Kind::Project, &id)])
        .await
        .unwrap()
        .version;
    let configured = store
        .read(&[
            ControlFilter::id(Kind::Project, &id),
            ControlFilter::all(Kind::Settings),
        ])
        .await
        .unwrap()
        .version;
    let global = store.read(&[]).await.unwrap().version;
    store
        .apply_versioned(
            &global,
            vec![put(
                Kind::Settings,
                "settings",
                None,
                json!({"id":"settings"}),
            )],
            vec![],
        )
        .await
        .unwrap();
    let message = put(
        Kind::Message,
        "answer",
        Some(&id),
        json!({"id":"answer","project_id":id,"parent_message_id":"root"}),
    );
    assert!(matches!(
        store
            .apply_versioned(&configured, vec![message.clone()], vec![])
            .await,
        Err(ControlStoreError::Conflict)
    ));
    store
        .apply_versioned(&project, vec![message], vec![])
        .await
        .unwrap();
}

#[tokio::test]
async fn project_commit_survives_catalog_projection_failure_and_rebuilds() {
    let temp = TempDir::new().unwrap();
    let id = format!("projection-{}", temp.path().display());
    let store = PortableSqliteControlStore::open(temp.path().join("catalog")).unwrap();
    create(&store, &temp.path().join("project"), &id).await;
    let version = store
        .read(&[ControlFilter::id(Kind::Project, &id)])
        .await
        .unwrap()
        .version;
    store.access(|state| { state.catalog.execute_batch("CREATE TRIGGER fail_projection BEFORE INSERT ON record_locations BEGIN SELECT RAISE(ABORT,'injected projection failure'); END;").unwrap(); Ok(()) }).unwrap();
    store
        .apply_versioned(
            &version,
            vec![put(
                Kind::Message,
                "saved",
                Some(&id),
                json!({"id":"saved","project_id":id,"parent_message_id":"root"}),
            )],
            vec![],
        )
        .await
        .unwrap();
    assert!(
        store
            .read(&[ControlFilter::message_ancestors("saved")])
            .await
            .unwrap_err()
            .to_string()
            .contains("PROJECT_INDEX_REBUILDING")
    );
    store
        .access(|state| {
            state
                .catalog
                .execute_batch("DROP TRIGGER fail_projection")
                .unwrap();
            Ok(())
        })
        .unwrap();
    assert_eq!(
        store
            .read(&[ControlFilter::message_ancestors("saved")])
            .await
            .unwrap()
            .records
            .len(),
        2
    );
}

#[tokio::test]
async fn failed_initialization_is_atomic_and_retryable() {
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
    let id = format!("atomic-{}", temp.path().display());
    let store = PortableSqliteControlStore::open(temp.path().join("catalog")).unwrap();
    let version = store.read(&[]).await.unwrap().version;
    let invalid = vec![
        put(
            Kind::Project,
            &id,
            Some(&id),
            json!({"id":id,"workdir":root}),
        ),
        put(
            Kind::Message,
            "bad",
            Some(&id),
            json!({"id":"bad","project_id":id,"parent_message_id":"absent"}),
        ),
    ];
    assert!(
        store
            .apply_versioned(&version, invalid, vec![])
            .await
            .is_err()
    );
    assert!(
        store
            .open_project(root.to_str().unwrap())
            .await
            .unwrap()
            .is_none()
    );
    create(&store, &root, &id).await;
    let current = store
        .read(&[ControlFilter::id(Kind::Project, &id)])
        .await
        .unwrap()
        .version;
    assert!(
        store
            .apply_versioned(
                &current,
                vec![put(
                    Kind::Message,
                    "bad",
                    Some(&id),
                    json!({"id":"bad","project_id":"foreign"})
                )],
                vec![]
            )
            .await
            .is_err()
    );
    assert_eq!(
        store
            .read(&[ControlFilter::project(Kind::Message, &id)])
            .await
            .unwrap()
            .records
            .len(),
        1
    );
}

#[cfg(unix)]
#[tokio::test]
async fn replaced_storage_is_not_written_through_an_old_connection() {
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("project");
    let id = format!("replacement-{}", temp.path().display());
    let store = PortableSqliteControlStore::open(temp.path().join("catalog")).unwrap();
    create(&store, &root, &id).await;
    let version = store
        .read(&[ControlFilter::id(Kind::Project, &id)])
        .await
        .unwrap()
        .version;
    std::fs::rename(root.join(".ait/project.lock"), root.join(".ait/old.lock")).unwrap();
    std::fs::File::create(root.join(".ait/project.lock")).unwrap();
    assert!(
        store
            .apply_versioned(&version, vec![], vec![])
            .await
            .unwrap_err()
            .to_string()
            .contains("WORKSPACE_INVALID")
    );
}
