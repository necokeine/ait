//! Native Codex -> durable fragments -> fork attachment -> restart contract.

use super::*;

async fn wait_for_prefix(execution: &AgentExecution, id: &str) {
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let page = execution
                .execute(
                    "agent.timeline.get.request",
                    json!({"agentId":id,"limit":0}),
                )
                .await
                .unwrap();
            if page["entries"]
                .as_array()
                .unwrap()
                .iter()
                .any(|entry| entry["item"]["text"] == "Echo: ")
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
}

async fn exported(execution: &AgentExecution, id: &str) -> String {
    execution
        .execute("agent.fork_context.request", json!({"agentId":id}))
        .await
        .unwrap()["attachment"]["text"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[tokio::test]
async fn completed_native_stream_exports_readable_context_before_and_after_restart() {
    let fixture = Fixture::new();
    let (execution, _) = worker(&fixture);
    let created = create(&execution, &fixture).await;
    let id = created["agentId"].as_str().unwrap();
    execution
        .execute(
            "agent.message.send.request",
            json!({"agentId":id,"text":"stream"}),
        )
        .await
        .unwrap();
    wait_for_prefix(&execution, id).await;
    execution
        .execute(
            "agent.message.send.request",
            json!({"agentId":id,"text":"continue","activeTurnBehavior":"steer"}),
        )
        .await
        .unwrap();
    execution
        .execute("agent.finish.wait.request", json!({"agentId":id}))
        .await
        .unwrap();
    let before = exported(&execution, id).await;
    execution.shutdown().await.unwrap();
    let (reopened, _) = worker(&fixture);
    let after = exported(&reopened, id).await;
    reopened.shutdown().await.unwrap();
    assert!(
        before.contains("[Assistant] Echo:\n[User] continue\n[Assistant] stream + continue\n"),
        "{before}"
    );
    assert_eq!(after, before);
    assert_eq!(before.matches("Echo:").count(), 1);
}
