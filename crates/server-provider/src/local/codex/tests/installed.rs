use super::*;

#[tokio::test]
#[ignore = "requires installed Codex authentication and makes one small model request"]
async fn installed_codex_completes_a_native_turn_and_reopens_history() {
    let root = tempfile::tempdir().unwrap();
    let client = CodexClient::new(
        std::env::var_os("AIT_SERVER_CODEX_BIN").map_or_else(|| "codex".into(), Into::into),
    );
    let spec = AgentSessionSpec {
        provider: "codex".to_owned(),
        cwd: root.path().to_str().unwrap().to_owned(),
        config: StoredAgentConfig::default(),
    };
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
                Some(AgentTurnEvent::Failed) => panic!("installed Codex turn failed"),
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
