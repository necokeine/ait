use super::*;

async fn presets(store: &PortableSqliteControlStore, agent: &str, provider: &str) {
    let version = store.read(&[]).await.unwrap().version;
    store.apply_versioned(&version,vec![
        put(Kind::Provider,provider,None,json!({"id":provider,"kind":"open_ai"})),
        put(Kind::Agent,agent,None,json!({"id":agent,"enabled":true,"owner_session_id":null,"config":{"provider_id":provider,"model":"test"}}))
    ],vec![]).await.unwrap();
}

#[tokio::test]
#[allow(
    clippy::too_many_lines,
    reason = "exercise a complete cross-catalog adoption and invalidation sequence"
)]
async fn foreign_configuration_and_cron_require_explicit_local_adoption() {
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("project");
    let id = format!("binding-{}", temp.path().display());
    let first = PortableSqliteControlStore::open(temp.path().join("first")).unwrap();
    presets(&first, "saved-agent", "saved-provider").await;
    create(&first, &root, &id).await;
    let read = first
        .read(&[ControlFilter::id(Kind::Project, &id)])
        .await
        .unwrap();
    let mut project = read.records[0].clone();
    project.value["default_agent_id"] = json!("saved-agent");
    first
        .apply_versioned(
            &read.version,
            vec![
                ControlChange::Put(project),
                put(
                    Kind::Cron,
                    "cron",
                    Some(&id),
                    json!({"id":"cron","project_id":id,"agent_id":"saved-agent","enabled":true}),
                ),
            ],
            vec![],
        )
        .await
        .unwrap();
    first.close_project(&id).await.unwrap();
    let second = PortableSqliteControlStore::open(temp.path().join("second")).unwrap();
    // An identical host ID never implicitly adopts the foreign saved preset.
    presets(&second, "saved-agent", "saved-provider").await;
    presets(&second, "local-agent", "saved-provider").await;
    let global = second.read(&[]).await.unwrap().version;
    second
        .apply_versioned(
            &global,
            vec![put(
                Kind::Provider,
                "saved-provider",
                None,
                json!({"id":"saved-provider","kind":"open_ai","url":"https://local.invalid"}),
            )],
            vec![],
        )
        .await
        .unwrap();
    second.open_project(root.to_str().unwrap()).await.unwrap();
    assert!(!second.project_can_recover(&id).await.unwrap());
    let filters = [
        ControlFilter::id(Kind::Project, &id),
        ControlFilter::id(Kind::Agent, "saved-agent"),
        ControlFilter::id(Kind::Provider, "saved-provider"),
    ];
    let read = second.read(&filters).await.unwrap();
    assert_eq!(
        read.records
            .iter()
            .find(|r| r.kind == Kind::Agent)
            .unwrap()
            .value["enabled"],
        false
    );
    let cron = second
        .read(&[ControlFilter::id(Kind::Cron, "cron")])
        .await
        .unwrap();
    assert_eq!(cron.records[0].value["enabled"], false);
    second
        .bind_project_agent(&read.version, &id, "saved-agent", "local-agent")
        .await
        .unwrap();
    let bound = second.read(&filters).await.unwrap();
    let agent = bound
        .records
        .iter()
        .find(|r| r.kind == Kind::Agent)
        .unwrap();
    assert_eq!(agent.value["enabled"], true);
    assert_eq!(agent.value["config"]["provider_id"], "saved-provider");
    assert_eq!(
        bound
            .records
            .iter()
            .find(|r| r.kind == Kind::Provider)
            .unwrap()
            .value["url"],
        "https://local.invalid"
    );
    let mut changed = agent.value.clone();
    changed["id"] = json!("local-agent");
    changed["config"]["model"] = json!("changed");
    let global = second.read(&[]).await.unwrap().version;
    second
        .apply_versioned(
            &global,
            vec![put(Kind::Agent, "local-agent", None, changed)],
            vec![],
        )
        .await
        .unwrap();
    assert_eq!(
        second
            .read(&filters)
            .await
            .unwrap()
            .records
            .iter()
            .find(|r| r.kind == Kind::Agent)
            .unwrap()
            .value["enabled"],
        false
    );
    let cron = second
        .read(&[ControlFilter::id(Kind::Cron, "cron")])
        .await
        .unwrap();
    let mut adopted = cron.records[0].clone();
    adopted.value["enabled"] = json!(true);
    second
        .apply_versioned(&cron.version, vec![ControlChange::Put(adopted)], vec![])
        .await
        .unwrap();
    assert_eq!(
        second
            .read(&[ControlFilter::id(Kind::Cron, "cron")])
            .await
            .unwrap()
            .records[0]
            .value["enabled"],
        true
    );
}

#[tokio::test]
async fn private_agents_live_with_sessions_and_native_reservations_span_catalogs() {
    let temp = TempDir::new().unwrap();
    let id = format!("native-{}", temp.path().display());
    let thread = format!("thread-{}", temp.path().display());
    let first = PortableSqliteControlStore::open(temp.path().join("first")).unwrap();
    create(&first, &temp.path().join("one"), &id).await;
    let version = first
        .read(&[ControlFilter::id(Kind::Project, &id)])
        .await
        .unwrap()
        .version;
    let session = json!({"id":"session","project_id":id,"agent_id":"private","source":{"type":"codex_thread","thread_id":thread}});
    first.apply_versioned(&version,vec![put(Kind::Session,"session",Some(&id),session),put(Kind::Agent,"private",None,json!({"id":"private","enabled":true,"owner_session_id":"session","config":{"provider_id":"provider"}}))],vec![]).await.unwrap();
    assert_eq!(
        first
            .read(&[ControlFilter::id(Kind::Agent, "private")])
            .await
            .unwrap()
            .records
            .len(),
        1
    );
    let native = first
        .read(&[ControlFilter::id(Kind::Project, &id)])
        .await
        .unwrap();
    assert!(
        first
            .bind_project_agent(&native.version, &id, "private", "private")
            .await
            .unwrap_err()
            .to_string()
            .contains("NATIVE_SOURCE_UNRESOLVED")
    );
    let second = PortableSqliteControlStore::open(temp.path().join("second")).unwrap();
    let other = format!("other-{id}");
    create(&second, &temp.path().join("two"), &other).await;
    let version = second
        .read(&[ControlFilter::id(Kind::Project, &other)])
        .await
        .unwrap()
        .version;
    let conflicting = put(
        Kind::Session,
        "session",
        Some(&other),
        json!({"id":"session","project_id":other,"source":{"type":"codex_thread","thread_id":thread}}),
    );
    assert!(
        second
            .apply_versioned(&version, vec![conflicting], vec![])
            .await
            .unwrap_err()
            .to_string()
            .contains("NATIVE_BINDING_CONFLICT")
    );
    first
        .access(|state| {
            let count: u64 = state
                .catalog
                .query_row("SELECT count(*) FROM agents WHERE id='private'", [], |r| {
                    r.get(0)
                })
                .unwrap();
            assert_eq!(count, 0);
            Ok(())
        })
        .unwrap();
}
