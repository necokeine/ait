use super::*;

#[tokio::test]
async fn creation_and_message_requests_deliver_rich_content_and_reject_bad_payloads_before_launch()
{
    let fixture = Fixture::new();
    let (execution, registry) = worker(&fixture);
    let images = json!([{"data":"aGVsbG8=","mimeType":"image/png"}]);
    let config = json!({"provider":"codex","cwd":fixture.cwd,"providerOptions":{"web_search":"live"},
        "mcpServers":{"docs":{"type":"stdio","command":"node"}},"toolPolicy":{"preapproved":[]}});
    assert!(
        execution
            .execute(
                "agent.create.request",
                json!({"config":config,"images":[{"data":"invalid","mimeType":"image/png"}]})
            )
            .await
            .is_err()
    );
    assert!(registry.list().unwrap().is_empty());
    assert!(!fixture.cwd.join("native-requests.jsonl").exists());
    let created = execution
        .execute(
            "agent.create.request",
            json!({"config":config,"images":images,"initialPrompt":"Inspect",
        "attachments":[{"type":"text","mimeType":"text/plain","text":"context"}]}),
        )
        .await
        .unwrap();
    let id = created["agentId"].as_str().unwrap();
    execution
        .execute("agent.finish.wait.request", json!({"agentId":id}))
        .await
        .unwrap();
    let response = execution
        .execute(
            "agent.message.send.request",
            json!({"agentId":id,"text":"", "images":images,
        "messageId":"client-input-1","outputSchema":{"type":"object"}}),
        )
        .await
        .unwrap();
    assert_eq!(response["accepted"], true);
    execution
        .execute("agent.finish.wait.request", json!({"agentId":id}))
        .await
        .unwrap();
    let requests = fixture.requests();
    let starts: Vec<_> = requests
        .iter()
        .filter(|request| request["method"] == "turn/start")
        .collect();
    assert_eq!(starts.len(), 2);
    assert_eq!(starts[0]["params"]["input"][2]["text"], "context");
    assert_eq!(
        starts[1]["params"]["input"][0]["url"],
        "data:image/png;base64,aGVsbG8="
    );
    assert_eq!(starts[1]["params"]["clientUserMessageId"], "client-input-1");
    assert_eq!(
        starts[1]["params"]["outputSchema"],
        json!({"type":"object"})
    );
    execution.shutdown().await.unwrap();
}
