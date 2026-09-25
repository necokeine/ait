use super::*;

async fn send(execution: &AgentExecution, id: &str, text: &str, steer: bool) -> Value {
    let mut params = json!({"agentId":id,"text":text});
    if steer {
        params["activeTurnBehavior"] = json!("steer");
    }
    execution
        .execute("agent.message.send.request", params)
        .await
        .unwrap()
}

async fn page(execution: &AgentExecution, id: &str) -> Value {
    execution
        .execute(
            "agent.timeline.get.request",
            json!({"agentId":id,"limit":0}),
        )
        .await
        .unwrap()
}

fn assistant(page: &Value) -> String {
    page["entries"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|entry| entry["item"]["type"] == "assistant_message")
        .map(|entry| entry["item"]["text"].as_str().unwrap())
        .collect()
}

#[tokio::test]
async fn streaming_is_durable_searchable_and_steering_stays_in_one_native_turn() {
    let fixture = Fixture::new();
    let (execution, registry) = worker(&fixture);
    let created = create(&execution, &fixture).await;
    let id = created["agentId"].as_str().unwrap();
    assert_eq!(created["agent"]["capabilities"]["supportsStreaming"], true);
    assert_eq!(
        send(&execution, id, "stream", false).await["accepted"],
        true
    );
    let partial = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let page = page(&execution, id).await;
            if assistant(&page) == "Echo: " {
                break page;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await;
    assert!(
        partial.is_ok(),
        "partial stream timed out: {}",
        page(&execution, id).await
    );
    let partial = partial.unwrap();
    assert_eq!(
        registry.get(id).unwrap().unwrap().last_status,
        server_domain::agent_runtime::AgentRuntimeStatus::Running
    );
    assert!(
        partial["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["item"]["detail"]["output"] == "offline output")
    );
    assert_eq!(
        send(&execution, id, "ordinary busy", false).await["accepted"],
        false
    );
    assert_eq!(
        send(&execution, id, "continue", true).await["accepted"],
        true
    );
    let finished = execution
        .execute("agent.finish.wait.request", json!({"agentId":id}))
        .await
        .unwrap();
    assert_eq!(finished["status"], "idle");
    assert_eq!(finished["lastMessage"], "Echo: stream + continue");
    let completed = page(&execution, id).await;
    assert_eq!(assistant(&completed), "Echo: stream + continue");
    assert_eq!(completed["epoch"], partial["epoch"]);
    let search = execution
        .execute(
            "agent.timeline.search.request",
            json!({"agentId":id,"query":"Echo: stream"}),
        )
        .await
        .unwrap();
    assert_eq!(search["locations"].as_array().unwrap().len(), 1);
    let requests = fixture.requests();
    assert_eq!(
        requests
            .iter()
            .filter(|r| r["method"] == "turn/start")
            .count(),
        1
    );
    let steer = requests
        .iter()
        .find(|r| r["method"] == "turn/steer")
        .unwrap();
    assert_eq!(
        steer["params"]["expectedTurnId"],
        partial["entries"][0]["turnId"]
    );
    execution.shutdown().await.unwrap();
    let (restarted, _) = worker(&fixture);
    assert_eq!(page(&restarted, id).await["entries"], completed["entries"]);
    restarted.shutdown().await.unwrap();
}

#[tokio::test]
async fn steer_rejection_preserves_turn_but_ambiguous_admission_closes_it() {
    for mode in [
        "steer-reject",
        "steer-completed",
        "steer-wrong-id",
        "steer-exit",
        "steer-ambiguous",
    ] {
        let fixture = Fixture::new();
        fixture.mode(mode);
        let (execution, registry) = worker(&fixture);
        let created = create(&execution, &fixture).await;
        let id = created["agentId"].as_str().unwrap();
        send(&execution, id, "hang", false).await;
        assert_eq!(
            send(&execution, id, "continue", true).await["accepted"],
            false,
            "{mode}"
        );
        let expected = match mode {
            "steer-reject" => {
                assert_eq!(
                    registry.get(id).unwrap().unwrap().last_status,
                    server_domain::agent_runtime::AgentRuntimeStatus::Running
                );
                execution
                    .execute("agent.cancel.request", json!({"agentId":id}))
                    .await
                    .unwrap();
                "idle"
            }
            "steer-completed" => "idle",
            _ => "error",
        };
        assert_eq!(
            execution
                .execute("agent.finish.wait.request", json!({"agentId":id}))
                .await
                .unwrap()["status"],
            expected,
            "{mode}"
        );
        assert_eq!(
            fixture
                .requests()
                .iter()
                .filter(|r| r["method"] == "turn/start")
                .count(),
            1
        );
        execution.shutdown().await.unwrap();
    }
}

#[tokio::test]
async fn steer_validation_preserves_permissions_and_idle_selection_starts_a_turn() {
    let fixture = Fixture::new();
    let (execution, _) = worker(&fixture);
    let created = create(&execution, &fixture).await;
    let id = created["agentId"].as_str().unwrap();
    assert_eq!(send(&execution, id, "hello", true).await["accepted"], true);
    execution
        .execute("agent.finish.wait.request", json!({"agentId":id}))
        .await
        .unwrap();
    send(&execution, id, "permit-command", false).await;
    let permission = super::controls::pending(&execution, id).await;
    for text in ["continue", "", "/review args"] {
        assert_eq!(send(&execution, id, text, true).await["accepted"], false);
    }
    assert_eq!(super::controls::pending(&execution, id).await, permission);
    assert_eq!(
        execution
            .execute(
                "agent.message.send.request",
                json!({"agentId":id,"text":"next","activeTurnBehavior":"interrupt"})
            )
            .await,
        Err(ErrorCode::InvalidMessage)
    );
    execution
        .execute(
            "agent.permission.resolve.request",
            json!({"agentId":id,"requestId":permission["id"],"response":{"behavior":"deny"}}),
        )
        .await
        .unwrap();
    execution
        .execute("agent.finish.wait.request", json!({"agentId":id}))
        .await
        .unwrap();
    assert!(
        !fixture
            .requests()
            .iter()
            .any(|r| r["method"] == "turn/steer")
    );
    execution.shutdown().await.unwrap();
}
