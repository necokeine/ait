use super::*;

mod paseo;

fn thread() -> Value {
    json!({"id":"native","cwd":"/tmp","createdAt":1_700_000_000,"updatedAt":1_700_000_010,
        "status":{"type":"idle"},"model":"offline-model","reasoningEffort":"high",
        "name":" Native title ","preview":" Prompt ","turns":[{"id":"turn","status":"completed",
        "items":[{"type":"agentMessage","id":"reply","text":"hello"}]}]})
}

#[test]
fn native_history_validates_complete_identity_and_preserves_configuration() {
    let parsed = history(&thread()).unwrap();
    assert_eq!(parsed.descriptor.title.as_deref(), Some("Native title"));
    assert_eq!(parsed.config.model.as_deref(), Some("offline-model"));
    assert_eq!(parsed.config.thinking_option_id.as_deref(), Some("high"));
    assert_eq!(parsed.entries.len(), 1);
    assert!(!parsed.active);
    for (pointer, value) in [
        ("/id", json!("")),
        ("/cwd", json!("relative")),
        ("/createdAt", json!("bad")),
        ("/updatedAt", json!(i64::MAX)),
        ("/status/type", json!("systemError")),
        ("/turns/0/status", json!("unknown")),
        ("/turns/0/items", json!(null)),
        ("/turns/0/id", json!("bad\nidentity")),
    ] {
        let mut invalid = thread();
        *invalid.pointer_mut(pointer).unwrap() = value;
        assert!(history(&invalid).is_err(), "{invalid}");
    }
    let mut partial = thread();
    partial["turns"][0]["itemsView"] = json!("summary");
    assert!(history(&partial).is_err());
    partial["turns"][0]["status"] = json!("inProgress");
    let parsed = history(&partial).unwrap();
    assert!(parsed.active);
    assert!(parsed.entries.is_empty());
    let mut duplicate = thread();
    duplicate["turns"]
        .as_array_mut()
        .unwrap()
        .push(thread()["turns"][0].clone());
    assert!(history(&duplicate).is_err());
}

#[cfg(unix)]
#[tokio::test]
async fn native_list_paginates_deduplicates_and_rejects_cursor_loops() {
    let fixture = crate::test_support::Fixture::new();
    let mut second = thread();
    second["id"] = json!("second");
    let mut child = thread();
    child["parentThreadId"] = json!("parent");
    let mut ephemeral = thread();
    ephemeral["ephemeral"] = json!(true);
    let pages = json!({"first":{"data":[thread(),child,ephemeral],"nextCursor":"next"},
        "next":{"data":[thread(),second],"nextCursor":null}});
    let path = fixture.cwd.join("session-pages.json");
    std::fs::write(&path, pages.to_string()).unwrap();
    let mut options = ListOptions {
        cwd: Some(fixture.spec().cwd),
        scan_limit: 10,
    };
    let client = fixture.client();
    assert_eq!(client.list_native(&options).await.unwrap().len(), 2);
    options.scan_limit = 1;
    assert_eq!(client.list_native(&options).await.unwrap().len(), 1);
    options.scan_limit = 0;
    assert!(client.list_native(&options).await.is_err());
    options.scan_limit = 10;
    let mut looped = pages;
    looped["next"]["nextCursor"] = json!("next");
    std::fs::write(&path, looped.to_string()).unwrap();
    assert!(client.list_native(&options).await.is_err());
    assert!(
        client
            .inspect_native("missing-session", fixture.cwd.to_str().unwrap())
            .await
            .is_err()
    );
    fixture.mode("wrong-thread");
    assert!(
        client
            .inspect_native("native", fixture.cwd.to_str().unwrap())
            .await
            .is_err()
    );
    assert!(fixture.requests().iter().all(|request| !matches!(
        request["method"].as_str(),
        Some("thread/start" | "thread/resume")
    )));
}
