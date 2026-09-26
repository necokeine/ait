use super::*;

#[tokio::test]
async fn plan_review_survives_restart_then_approval_disables_plan_mode_and_submits_once() {
    let fixture = Fixture::new();
    fixture.mode("workflows");
    let (execution, _) = worker(&fixture);
    let created = create(&execution, &fixture).await;
    let id = created["agentId"].as_str().unwrap();
    execution
        .execute(
            "agent.config.apply.request",
            json!({"agentId":id,"config":{"featureValues":{"plan_mode":true}}}),
        )
        .await
        .unwrap();
    execution
        .execute(
            "agent.message.send.request",
            json!({"agentId":id,"text":"propose-plan"}),
        )
        .await
        .unwrap();
    execution
        .execute("agent.finish.wait.request", json!({"agentId":id}))
        .await
        .unwrap();
    execution.shutdown().await.unwrap();
    let (execution, registry) = worker(&fixture);
    let snapshot = execution
        .execute("agent.get.request", json!({"agentId":id}))
        .await
        .unwrap();
    let request = &snapshot["agent"]["pendingPermissions"][0];
    assert_eq!(request["name"], "CodexPlanApproval");
    let response = json!({"agentId":id,"requestId":request["id"],"response":{"behavior":"allow"}});
    execution
        .execute("agent.permission.resolve.request", response.clone())
        .await
        .unwrap();
    execution
        .execute("agent.finish.wait.request", json!({"agentId":id}))
        .await
        .unwrap();
    assert!(
        execution
            .execute("agent.permission.resolve.request", response)
            .await
            .is_err()
    );
    let record = registry.get(id).unwrap().unwrap();
    assert_eq!(
        record.config.unwrap().feature_values.unwrap()["plan_mode"],
        false
    );
    let requests = fixture.requests();
    let starts: Vec<_> = requests
        .iter()
        .filter(|request| request["method"] == "turn/start")
        .collect();
    assert_eq!(starts.len(), 2);
    assert_eq!(starts[0]["params"]["collaborationMode"]["mode"], "plan");
    assert_eq!(starts[1]["params"]["collaborationMode"]["mode"], "default");
    assert!(
        starts[1]["params"]["input"][0]["text"]
            .as_str()
            .unwrap()
            .contains("Approved plan:\n\n1. Implement")
    );
    assert!(
        record.persistence.unwrap().metadata.unwrap()["controlNotes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["item"]["metadata"]["resolution"] == "approved")
    );
    execution.shutdown().await.unwrap();
}

#[tokio::test]
async fn dismissing_or_replacing_a_plan_does_not_launch_an_implementation_turn() {
    let fixture = Fixture::new();
    fixture.mode("workflows");
    let (execution, _) = worker(&fixture);
    let created = create(&execution, &fixture).await;
    let id = created["agentId"].as_str().unwrap();
    execution
        .execute(
            "agent.config.apply.request",
            json!({"agentId":id,"config":{"featureValues":{"plan_mode":true}}}),
        )
        .await
        .unwrap();
    for dismiss in [true, false] {
        execution
            .execute(
                "agent.message.send.request",
                json!({"agentId":id,"text":"propose-plan"}),
            )
            .await
            .unwrap();
        execution
            .execute("agent.finish.wait.request", json!({"agentId":id}))
            .await
            .unwrap();
        let snapshot = execution
            .execute("agent.get.request", json!({"agentId":id}))
            .await
            .unwrap();
        if dismiss {
            execution.execute("agent.permission.resolve.request", json!({"agentId":id,"requestId":snapshot["agent"]["pendingPermissions"][0]["id"],"response":{"behavior":"deny"}})).await.unwrap();
        } else {
            execution
                .execute(
                    "agent.message.send.request",
                    json!({"agentId":id,"text":"New instructions"}),
                )
                .await
                .unwrap();
            execution
                .execute("agent.finish.wait.request", json!({"agentId":id}))
                .await
                .unwrap();
        }
        let snapshot = execution
            .execute("agent.get.request", json!({"agentId":id}))
            .await
            .unwrap();
        assert_eq!(snapshot["agent"]["pendingPermissions"], json!([]));
    }
    assert_eq!(
        fixture
            .requests()
            .iter()
            .filter(|request| request["method"] == "turn/start")
            .count(),
        3
    );
    execution.shutdown().await.unwrap();
}
