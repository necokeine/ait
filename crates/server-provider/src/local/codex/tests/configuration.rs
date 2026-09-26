use super::*;

#[tokio::test]
async fn advanced_options_override_presets_and_reconfigure_the_same_native_thread() {
    let fixture = Fixture::new();
    let client = fixture.client();
    let mut spec = fixture.spec();
    spec.config = serde_json::from_value(json!({"modeId":"read-only", "providerOptions":{
        "approval_policy":{"granular":{"sandbox_approval":true,"mcp_elicitations":true}},
        "sandbox_mode":"workspace-write", "sandbox_workspace_write":{
            "writable_roots":["/tmp/fixture"],"network_access":true,"exclude_slash_tmp":true}},
        "mcpServers":{"docs":{"type":"http","url":"https://example.com/mcp"}},
        "toolPolicy":{"preapproved":[{"kind":"mcp","server":"docs","tool":"search"}]}}))
    .unwrap();
    let mut session = client.create_session(&spec).await.unwrap();
    let handle = session.persistence();
    session
        .start_turn("configured", &spec.config)
        .await
        .unwrap();
    assert!(matches!(
        terminal(session.as_mut()).await.unwrap(),
        AgentTurnEvent::Completed(_)
    ));
    spec.config.mcp_servers = None;
    spec.config.provider_options = None;
    spec.config.tool_policy = None;
    session.start_turn("cleared", &spec.config).await.unwrap();
    assert!(matches!(
        terminal(session.as_mut()).await.unwrap(),
        AgentTurnEvent::Completed(_)
    ));
    assert_eq!(session.persistence(), handle);
    session.close().await.unwrap();
    let requests = fixture.requests();
    let start = &requests
        .iter()
        .find(|request| request["method"] == "thread/start")
        .unwrap()["params"];
    assert_eq!(
        start["approvalPolicy"]["granular"]["sandbox_approval"],
        true
    );
    assert_eq!(start["sandbox"], "workspace-write");
    assert_eq!(
        start["config"]["mcp_servers"]["docs"]["tools"]["search"]["approval_mode"],
        "approve"
    );
    let turns: Vec<_> = requests
        .iter()
        .filter(|request| request["method"] == "turn/start")
        .collect();
    assert_eq!(
        turns[0]["params"]["sandboxPolicy"],
        json!({"type":"workspaceWrite","networkAccess":true,"excludeSlashTmp":true,"writableRoots":["/tmp/fixture"]})
    );
    assert_eq!(turns[1]["params"]["sandboxPolicy"]["type"], "readOnly");
    let resumed = &requests
        .iter()
        .find(|request| request["method"] == "thread/resume")
        .unwrap()["params"];
    assert_eq!(resumed["config"], json!({}));
    assert_eq!(resumed["approvalPolicy"], "never");
}

#[tokio::test]
async fn thread_usage_notifications_without_turn_identity_are_retained() {
    let fixture = Fixture::new();
    let client = fixture.client();
    let spec = fixture.spec();
    let mut session = client.create_session(&spec).await.unwrap();
    session.start_turn("usage", &spec.config).await.unwrap();
    let AgentTurnEvent::Usage(usage) = terminal(session.as_mut()).await.unwrap() else {
        panic!("expected native usage before completion");
    };
    assert_eq!(usage.context_window_used_tokens, Some(107));
    assert_eq!(usage.cached_input_tokens, Some(30));
    assert_eq!(usage.context_window_max_tokens, Some(200_000));
    assert!(matches!(
        terminal(session.as_mut()).await.unwrap(),
        AgentTurnEvent::Completed(_)
    ));
    session.close().await.unwrap();
}
