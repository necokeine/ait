use super::*;

#[tokio::test]
async fn native_file_and_combined_rewind_preserve_the_source_conversation_over_websocket() {
    let fixture = fixture();
    let state = fixture.root.path().join("state");
    let log = fixture.root.path().join("server.log");
    let mut process = start_with_path(&state, &log, Some(&fixture.path));
    let address = ready(&mut process, &log).await;
    let mut client = connect(&address, METHODS).await;
    request(
        &mut client,
        "workspace.open.request",
        json!({"cwd":fixture.cwd}),
    )
    .await;
    let created = request(
        &mut client,
        "agent.create.request",
        json!({"config":{"provider":"claude","cwd":fixture.cwd,"modeId":"default"}}),
    )
    .await;
    let id = created["result"]["agentId"].as_str().unwrap().to_owned();
    let original = created["result"]["agent"]["persistence"]["sessionId"].clone();
    for text in ["first", "second"] {
        assert_eq!(
            request(
                &mut client,
                "agent.message.send.request",
                json!({"agentId":id,"text":text,"messageId":text})
            )
            .await["result"]["accepted"],
            true
        );
        request(
            &mut client,
            "agent.finish.wait.request",
            json!({"agentId":id}),
        )
        .await;
    }
    let page = request(
        &mut client,
        "agent.timeline.get.request",
        json!({"agentId":id}),
    )
    .await;
    let target = page["result"]["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["item"]["clientMessageId"] == "second")
        .unwrap()["item"]["messageId"]
        .clone();
    assert_rewind(&mut client, &fixture, &id, &target, &original).await;
    terminate(&mut process).await;
}

async fn assert_rewind(
    client: &mut super::super::transport::Socket,
    fixture: &super::super::native::NativeFixture,
    id: &str,
    target: &serde_json::Value,
    original: &serde_json::Value,
) {
    std::fs::write(fixture.cwd.join("tracked.txt"), "changed").unwrap();
    assert_eq!(
        request(
            client,
            "agent.rewind.request",
            json!({"agentId":id,"messageId":target,"mode":"files"})
        )
        .await["result"]["ok"],
        true
    );
    assert_eq!(
        std::fs::read_to_string(fixture.cwd.join("tracked.txt")).unwrap(),
        "checkpoint restored"
    );
    let snapshot = request(client, "agent.get.request", json!({"agentId":id})).await;
    assert_eq!(
        snapshot["result"]["agent"]["runtimeInfo"]["sessionId"], *original,
        "{snapshot}"
    );
    std::fs::write(fixture.cwd.join("tracked.txt"), "changed again").unwrap();
    assert_eq!(
        request(
            client,
            "agent.rewind.request",
            json!({"agentId":id,"messageId":target,"mode":"both"})
        )
        .await["result"]["ok"],
        true
    );
    assert_eq!(
        std::fs::read_to_string(fixture.cwd.join("tracked.txt")).unwrap(),
        "checkpoint restored"
    );
    let snapshot = request(client, "agent.get.request", json!({"agentId":id})).await;
    assert_ne!(
        snapshot["result"]["agent"]["runtimeInfo"]["sessionId"],
        *original
    );
    let page = request(client, "agent.timeline.get.request", json!({"agentId":id})).await;
    assert!(
        !page["result"]["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["item"]["text"] == "second")
    );
    let sent = request(
        client,
        "agent.message.send.request",
        json!({"agentId":id,"text":"after rewind"}),
    )
    .await;
    assert_eq!(sent["result"]["accepted"], true);
    assert_eq!(
        request(client, "agent.finish.wait.request", json!({"agentId":id})).await["result"]["lastMessage"],
        "Claude: after rewind"
    );
}
