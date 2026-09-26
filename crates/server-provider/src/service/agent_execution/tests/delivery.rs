use super::*;

#[tokio::test]
async fn interrupt_delivery_drains_new_work_and_retries_are_durable_and_payload_bound() {
    let fixture = Fixture::new();
    let (execution, _) = worker(&fixture);
    let created = create(&execution, &fixture).await;
    let id = created["agentId"].as_str().unwrap();
    let send = json!({"agentId":id,"text":"hang","messageId":"first"});
    assert_eq!(
        execution
            .execute("agent.message.send.request", send.clone())
            .await
            .unwrap()["accepted"],
        true
    );
    assert_eq!(
        execution
            .execute("agent.message.send.request", send)
            .await
            .unwrap()["accepted"],
        true
    );
    let replacement = json!({"agentId":id,"text":"replacement","messageId":"second"});
    assert_eq!(
        execution
            .execute("agent.message.send.request", replacement.clone())
            .await
            .unwrap()["accepted"],
        true
    );
    let finished = execution
        .execute("agent.finish.wait.request", json!({"agentId":id}))
        .await
        .unwrap();
    assert_eq!(finished["lastMessage"], "Echo: replacement");
    assert_eq!(
        fixture
            .requests()
            .iter()
            .filter(|r| r["method"] == "turn/start")
            .count(),
        2
    );
    assert_eq!(
        fixture
            .requests()
            .iter()
            .filter(|r| r["method"] == "turn/interrupt")
            .count(),
        1
    );
    execution.shutdown().await.unwrap();
    let (restarted, _) = worker(&fixture);
    assert_eq!(
        restarted
            .execute("agent.message.send.request", replacement)
            .await
            .unwrap()["accepted"],
        true
    );
    assert_eq!(
        restarted
            .execute(
                "agent.message.send.request",
                json!({"agentId":id,"text":"changed","messageId":"second"})
            )
            .await
            .unwrap()["accepted"],
        false
    );
    assert_eq!(
        fixture
            .requests()
            .iter()
            .filter(|r| r["method"] == "turn/start")
            .count(),
        2
    );
    restarted.shutdown().await.unwrap();
}

#[tokio::test]
async fn voice_exclusivity_blocks_interrupt_and_steer_without_reserving_or_delivering_input() {
    let fixture = Fixture::new();
    let (execution, _) = worker(&fixture);
    let created = create(&execution, &fixture).await;
    let id = created["agentId"].as_str().unwrap();
    let accepted = json!({"agentId":id,"text":"completed before voice","messageId":"before-voice"});
    execution
        .execute("agent.message.send.request", accepted.clone())
        .await
        .unwrap();
    execution
        .execute("agent.finish.wait.request", json!({"agentId":id}))
        .await
        .unwrap();
    assert_eq!(
        execution
            .execute("internal.voice.send", json!({"agentId":id,"text":"hang"}))
            .await
            .unwrap()["accepted"],
        true
    );
    assert_eq!(
        execution
            .execute("agent.message.send.request", accepted)
            .await
            .unwrap()["accepted"],
        true
    );
    for behavior in ["interrupt", "steer"] {
        assert_eq!(
            execution
                .execute(
                    "agent.message.send.request",
                    json!({"agentId":id,"text":"blocked","activeTurnBehavior":behavior})
                )
                .await
                .unwrap()["accepted"],
            false
        );
    }
    assert!(
        !fixture
            .requests()
            .iter()
            .any(|r| r["method"] == "turn/interrupt" || r["method"] == "turn/steer")
    );
    assert!(!execution.timeline().has_queued_input(id).unwrap());
    execution.shutdown().await.unwrap();
}

#[tokio::test]
async fn startup_dispatches_only_unclaimed_fifo_inputs_and_keeps_uncertain_receipts_fenced() {
    use crate::protocol::prompt::AgentPrompt;
    let fixture = Fixture::new();
    let (execution, _) = worker(&fixture);
    let created = create(&execution, &fixture).await;
    let id = created["agentId"].as_str().unwrap();
    let timeline = execution.timeline();
    execution.shutdown().await.unwrap();
    let prompt = AgentPrompt {
        client_message_id: Some("uncertain".into()),
        ..AgentPrompt::text("do not replay")
    };
    timeline
        .reserve_input(id, "uncertain", &prompt, "interrupt")
        .unwrap();
    timeline.claim_input(id, "uncertain").unwrap();
    timeline
        .reserve_input(id, "safe", &AgentPrompt::text("safe recovery"), "interrupt")
        .unwrap();
    let (restarted, _) = worker(&fixture);
    let finished = restarted
        .execute("agent.finish.wait.request", json!({"agentId":id}))
        .await
        .unwrap();
    assert_eq!(finished["lastMessage"], "Echo: safe recovery");
    assert_eq!(
        restarted
            .execute(
                "agent.message.send.request",
                json!({"agentId":id,"text":"do not replay","messageId":"uncertain"})
            )
            .await
            .unwrap()["accepted"],
        false
    );
    assert_eq!(
        fixture
            .requests()
            .iter()
            .filter(|r| r["method"] == "turn/start")
            .count(),
        1
    );
    restarted.shutdown().await.unwrap();
}

#[tokio::test]
async fn native_commands_are_idempotent_and_do_not_interrupt_a_busy_foreground_turn() {
    let fixture = Fixture::new();
    fixture.mode("workflows");
    let (execution, _) = worker(&fixture);
    let created = create(&execution, &fixture).await;
    let id = created["agentId"].as_str().unwrap();
    execution
        .execute(
            "agent.message.send.request",
            json!({"agentId":id,"text":"hang"}),
        )
        .await
        .unwrap();
    for (message, text) in [("goal-pause", "/goal pause"), ("compact-one", "/compact")] {
        let request = json!({"agentId":id,"text":text,"messageId":message});
        assert_eq!(
            execution
                .execute("agent.message.send.request", request.clone())
                .await
                .unwrap()["accepted"],
            true
        );
        assert_eq!(
            execution
                .execute("agent.message.send.request", request)
                .await
                .unwrap()["accepted"],
            true
        );
    }
    let requests = fixture.requests();
    assert!(
        !requests
            .iter()
            .any(|request| request["method"] == "turn/interrupt")
    );
    assert_eq!(
        requests
            .iter()
            .filter(|request| request["method"] == "turn/start")
            .count(),
        1
    );
    assert_eq!(
        requests
            .iter()
            .filter(|request| request["method"] == "thread/goal/set")
            .count(),
        1
    );
    assert_eq!(
        requests
            .iter()
            .filter(|request| request["method"] == "thread/compact/start")
            .count(),
        1
    );
    assert!(requests.iter().any(|request| {
        request["method"] == "thread/goal/set"
            && request["argv"]
                .as_array()
                .unwrap()
                .contains(&json!("goals"))
    }));
    execution
        .execute("agent.cancel.request", json!({"agentId":id}))
        .await
        .unwrap();
    execution
        .execute("agent.finish.wait.request", json!({"agentId":id}))
        .await
        .unwrap();
    execution.shutdown().await.unwrap();
    let (execution, _) = worker(&fixture);
    let history = execution
        .execute("agent.timeline.get.request", json!({"agentId":id}))
        .await
        .unwrap();
    assert!(
        history["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["item"]["text"] == "Goal paused.")
    );
    execution.shutdown().await.unwrap();
}

#[tokio::test]
async fn native_goal_continuations_acquire_and_release_the_host_foreground_turn() {
    let fixture = Fixture::new();
    fixture.mode("workflows");
    let (execution, registry) = worker(&fixture);
    let created = create(&execution, &fixture).await;
    let id = created["agentId"].as_str().unwrap();
    assert_eq!(
        execution
            .execute(
                "agent.message.send.request",
                json!({"agentId":id,"text":"/goal autonomous fixture"})
            )
            .await
            .unwrap()["accepted"],
        true
    );
    let finished = execution
        .execute("agent.finish.wait.request", json!({"agentId":id}))
        .await
        .unwrap();
    assert_eq!(finished["lastMessage"], "Echo: Goal completed");
    assert_eq!(
        registry.get(id).unwrap().unwrap().last_status,
        server_domain::agent_runtime::AgentRuntimeStatus::Idle
    );
    assert!(
        !fixture
            .requests()
            .iter()
            .any(|request| request["method"] == "turn/start")
    );
    execution.shutdown().await.unwrap();
}

#[tokio::test]
async fn failed_queued_admission_does_not_block_an_independent_agent() {
    use crate::protocol::prompt::AgentPrompt;
    let fixture = Fixture::new();
    let (execution, _) = worker(&fixture);
    let first = create(&execution, &fixture).await;
    let second = create(&execution, &fixture).await;
    let first = first["agentId"].as_str().unwrap();
    let second = second["agentId"].as_str().unwrap();
    let timeline = execution.timeline();
    execution.shutdown().await.unwrap();
    timeline
        .reserve_input(
            first,
            "invalid-command",
            &AgentPrompt::text("/missing-native-command"),
            "interrupt",
        )
        .unwrap();
    timeline
        .reserve_input(
            second,
            "valid-input",
            &AgentPrompt::text("independent input"),
            "interrupt",
        )
        .unwrap();
    let (restarted, registry) = worker(&fixture);
    let finished = restarted
        .execute("agent.finish.wait.request", json!({"agentId":second}))
        .await
        .unwrap();
    assert_eq!(finished["lastMessage"], "Echo: independent input");
    let failed = registry.get(first).unwrap().unwrap();
    assert_eq!(
        failed.last_status,
        server_domain::agent_runtime::AgentRuntimeStatus::Error
    );
    assert!(failed.requires_attention);
    assert!(!timeline.has_queued_input(first).unwrap());
    restarted.shutdown().await.unwrap();
}
