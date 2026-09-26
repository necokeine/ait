use super::*;

mod plans;

#[tokio::test]
async fn completed_question_survives_restart_and_admits_one_validated_answer() {
    let fixture = Fixture::new();
    let (execution, registry) = worker(&fixture);
    let created = create(&execution, &fixture).await;
    let id = created["agentId"].as_str().unwrap();
    execution
        .execute(
            "agent.message.send.request",
            json!({"agentId":id,"text":"async-question"}),
        )
        .await
        .unwrap();
    execution
        .execute("agent.finish.wait.request", json!({"agentId":id}))
        .await
        .unwrap();
    assert_eq!(
        registry.get(id).unwrap().unwrap().attention_reason,
        Some(server_domain::agent_runtime::AgentAttentionReason::Permission)
    );
    execution.shutdown().await.unwrap();
    let (execution, _) = worker(&fixture);
    let snapshot = execution
        .execute("agent.get.request", json!({"agentId":id}))
        .await
        .unwrap();
    let request = snapshot["agent"]["pendingPermissions"][0]["id"]
        .as_str()
        .unwrap();
    assert!(
        execution
            .execute(
                "agent.permission.resolve.request",
                json!({"agentId":id,"requestId":request,
        "response":{"behavior":"allow","updatedInput":{"answers":{}}}})
            )
            .await
            .is_err()
    );
    let response = json!({"agentId":id,"requestId":request,"response":{"behavior":"allow","updatedInput":{"answers":{"Question 1":"Rust"}}}});
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
    let requests = fixture.requests();
    let starts: Vec<_> = requests
        .iter()
        .filter(|request| request["method"] == "turn/start")
        .collect();
    assert_eq!(starts.len(), 2);
    assert_eq!(
        starts[1]["params"]["input"][0]["text"],
        "Answers to your questions:\n\nWhich runtime?\nRust"
    );
    assert!(
        !requests
            .iter()
            .any(|request| request["method"] == "turn/interrupt")
    );
    execution.shutdown().await.unwrap();
}

#[tokio::test]
async fn busy_question_answer_steers_without_interrupt_and_rejection_leaves_it_pending() {
    let fixture = Fixture::new();
    let (execution, _) = worker(&fixture);
    let created = create(&execution, &fixture).await;
    let id = created["agentId"].as_str().unwrap();
    execution
        .execute(
            "agent.message.send.request",
            json!({"agentId":id,"text":"async-question-running"}),
        )
        .await
        .unwrap();
    let request = tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            let snapshot = execution
                .execute("agent.get.request", json!({"agentId":id}))
                .await
                .unwrap();
            if let Some(request) = snapshot["agent"]["pendingPermissions"][0]["id"].as_str() {
                break request.to_owned();
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    let answer = json!({"agentId":id,"requestId":request,"response":{"behavior":"allow","updatedInput":{"answers":{"Question 1":"Rust"}}}});
    std::fs::write(
        std::path::Path::new(&fixture.cwd).join("reject-steer"),
        "steer-rejected",
    )
    .unwrap();
    assert!(
        execution
            .execute("agent.permission.resolve.request", answer.clone())
            .await
            .is_err()
    );
    assert!(
        !fixture
            .requests()
            .iter()
            .any(|request| request["method"] == "turn/interrupt")
    );
    std::fs::remove_file(std::path::Path::new(&fixture.cwd).join("reject-steer")).unwrap();
    execution
        .execute("agent.permission.resolve.request", answer)
        .await
        .unwrap();
    execution
        .execute("agent.finish.wait.request", json!({"agentId":id}))
        .await
        .unwrap();
    assert_eq!(
        fixture
            .requests()
            .iter()
            .filter(|request| request["method"] == "turn/start")
            .count(),
        1
    );
    execution.shutdown().await.unwrap();
}
