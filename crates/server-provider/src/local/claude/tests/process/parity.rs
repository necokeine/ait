use super::*;

#[tokio::test]
async fn autonomous_parent_output_starts_a_new_turn_and_client_ids_survive_history() {
    let (_root, client, spec) = fixture();
    let mut session = client.create_session(&spec).await.unwrap();
    let prompt = crate::protocol::prompt::AgentPrompt {
        client_message_id: Some("arbitrary-ui-id".to_owned()),
        ..crate::protocol::prompt::AgentPrompt::text("autonomous")
    };
    session.start_input(&prompt, &spec.config).await.unwrap();
    let mut started = 0;
    let mut completed = Vec::new();
    let mut client_id = None;
    while completed.len() < 2 {
        match next(session.as_mut()).await.unwrap() {
            AgentTurnEvent::Started(_) => started += 1,
            AgentTurnEvent::Completed(text) => completed.push(text.unwrap()),
            AgentTurnEvent::Timeline(entry) if entry.item["type"] == "user_message" => {
                client_id = entry.item["clientMessageId"].as_str().map(str::to_owned);
            }
            AgentTurnEvent::Failed => panic!("native autonomous output failed"),
            _ => {}
        }
    }
    assert_eq!(started, 1);
    assert_eq!(completed, ["Foreground finished", "Autonomous follow-up"]);
    assert_eq!(client_id.as_deref(), Some("arbitrary-ui-id"));
    let handle = session.persistence().unwrap();
    session.close().await.unwrap();
    let history = client.history(&handle, &spec.cwd).await.unwrap();
    assert!(
        history
            .iter()
            .any(|entry| entry.item["clientMessageId"] == "arbitrary-ui-id")
    );
}

#[tokio::test]
#[ignore = "requires installed Claude authentication and makes one small model request"]
async fn installed_claude_completes_a_native_turn_and_reopens_history() {
    let root = tempfile::tempdir().unwrap();
    let client = ClaudeClient::new(
        std::env::var_os("AIT_SERVER_CLAUDE_BIN").map_or_else(|| "claude".into(), Into::into),
    );
    let spec = super::super::spec(root.path());
    let mut session = client.create_session(&spec).await.unwrap();
    session
        .start_turn(
            "Reply exactly AIT_PROVIDER_SMOKE_OK. Do not use tools.",
            &spec.config,
        )
        .await
        .unwrap();
    let result = tokio::time::timeout(Duration::from_secs(90), async {
        loop {
            match session.poll_turn().unwrap() {
                Some(AgentTurnEvent::Completed(text)) => break text.unwrap(),
                Some(AgentTurnEvent::Failed) => panic!("installed Claude turn failed"),
                _ => tokio::time::sleep(Duration::from_millis(20)).await,
            }
        }
    })
    .await
    .unwrap();
    assert!(result.contains("AIT_PROVIDER_SMOKE_OK"));
    let handle = session.persistence().unwrap();
    session.close().await.unwrap();
    let history = client.history(&handle, &spec.cwd).await.unwrap();
    assert!(history.iter().any(|entry| {
        entry.item["text"]
            .as_str()
            .is_some_and(|text| text.contains("AIT_PROVIDER_SMOKE_OK"))
    }));
}

#[tokio::test]
async fn structured_results_absent_from_native_jsonl_survive_resume() {
    let (_root, client, spec) = fixture();
    let mut session = client.create_session(&spec).await.unwrap();
    session
        .start_turn("structured", &spec.config)
        .await
        .unwrap();
    assert!(finish(session.as_mut()).await.contains("42"));
    let handle = session.persistence().unwrap();
    session.close().await.unwrap();
    let history = client.history(&handle, &spec.cwd).await.unwrap();
    let result = history
        .iter()
        .find(|entry| {
            entry.item["messageId"]
                .as_str()
                .is_some_and(|id| id.starts_with("result:"))
        })
        .unwrap();
    assert!(result.item["text"].as_str().unwrap().contains("42"));
    let mut resumed = client
        .resume_session(&handle, &spec, AgentResumePurpose::Interactive)
        .await
        .unwrap();
    resumed.start_turn("next", &spec.config).await.unwrap();
    assert_eq!(finish(resumed.as_mut()).await, "Claude: next");
    let history = client
        .history(&resumed.persistence().unwrap(), &spec.cwd)
        .await
        .unwrap();
    assert_eq!(
        history
            .iter()
            .filter(|entry| entry.key == result.key)
            .count(),
        1
    );
    resumed.close().await.unwrap();
}
