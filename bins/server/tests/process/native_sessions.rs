use std::path::Path;

use serde_json::{Value, json};

use super::transport::{Socket, connect, connect_as, receive, request};
use super::{ready, start_with_path, terminate};

const METHODS: &[&str] = &[
    "provider.sessions.recent.list.request",
    "agent.import.request",
    "agent.refresh.request",
    "agent.fork_context.request",
    "agent.timeline.get.request",
    "agent.timeline.append.request",
    "agent.timeline.set_subscription.request",
    "agent.get.request",
    "agent.archive.request",
    "agent.update.request",
    "agent.message.send.request",
    "agent.finish.wait.request",
    "workspace.list.request",
    "agent.list.request",
];

fn seed(cwd: &Path, id: &str, extra: &Value) {
    let mut metadata = json!({"id":id,"cwd":cwd,"name":"Native title","preview":"Existing prompt",
        "createdAt":1_700_000_000,"updatedAt":1_700_000_100,"model":"import-model",
        "reasoningEffort":"high","status":{"type":"idle"}});
    metadata
        .as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    std::fs::write(
        cwd.join(format!("native-session-{id}.json")),
        metadata.to_string(),
    )
    .unwrap();
    history(cwd, id, "Existing reply");
}

fn history(cwd: &Path, id: &str, reply: &str) {
    std::fs::write(cwd.join(format!("native-history-{id}.json")),json!([{
        "id":"original-turn","status":"completed","startedAt":1_700_000_000,
        "items":[{"type":"userMessage","id":"user","content":[{"type":"text","text":"Existing prompt"}]},
        {"type":"agentMessage","id":"reply","text":reply}]}]).to_string()).unwrap();
}

fn import(cwd: &Path, id: &str) -> Value {
    json!({"providerId":"codex","providerHandleId":id,"cwd":cwd})
}

async fn success(client: &mut Socket, method: &str, params: Value) -> Value {
    let response = request(client, method, params).await;
    assert_eq!(response["type"], "response", "{method}: {response}");
    response["result"].clone()
}

#[tokio::test]
async fn native_discovery_import_archive_restore_and_restart_preserve_identity() {
    let fixture = super::native::NativeFixture::new();
    seed(&fixture.cwd, "native", &json!({}));
    seed(
        &fixture.cwd,
        "metadata",
        &json!({"preview":"Generate metadata for a coding agent based on the user prompt. ignored"}),
    );
    let state = fixture.root.path().join("state");
    let log = fixture.root.path().join("server.log");
    let mut process = start_with_path(&state, &log, Some(&fixture.path));
    let address = ready(&mut process, &log).await;
    let mut client = connect(&address, METHODS).await;
    assert_recent_validation(&mut client, &fixture.cwd).await;
    let discovered = success(&mut client,"provider.sessions.recent.list.request",json!({"cwd":fixture.cwd,"query":"NATIVE","limit":10,"providers":["codex"],"since":"2023-01-01T00:00:00Z"})).await;
    assert_eq!(
        discovered["entries"].as_array().unwrap().len(),
        1,
        "{discovered}"
    );
    assert_eq!(discovered["entries"][0]["providerHandleId"], "native");
    let created = success(
        &mut client,
        "agent.import.request",
        import(&fixture.cwd, "native"),
    )
    .await;
    let id = created["agentId"].as_str().unwrap().to_owned();
    assert_eq!(created["status"], "agent_resumed");
    assert_eq!(created["timelineSize"], 2);
    assert_eq!(created["agent"]["model"], "import-model", "{created}");
    assert!(created["agent"]["workspaceId"].is_string());
    let requests = std::fs::read_to_string(fixture.cwd.join("native-requests.jsonl")).unwrap();
    assert!(!requests.contains("thread/resume"));
    assert!(!requests.contains("thread/start"));
    assert_eq!(
        request(
            &mut client,
            "agent.import.request",
            import(&fixture.cwd, "native")
        )
        .await["code"],
        "invalid_message"
    );
    let filtered = success(
        &mut client,
        "provider.sessions.recent.list.request",
        json!({"cwd":fixture.cwd}),
    )
    .await;
    assert_eq!(filtered["entries"], json!([]));
    assert_eq!(filtered["filteredAlreadyImportedCount"], 1);
    let page = success(
        &mut client,
        "agent.timeline.get.request",
        json!({"agentId":id}),
    )
    .await;
    let epoch = page["epoch"].clone();
    let attachment = success(
        &mut client,
        "agent.fork_context.request",
        json!({"agentId":id,"boundaryMessageId":"reply"}),
    )
    .await;
    assert_eq!(attachment["itemCount"], 2);
    assert!(
        attachment["attachment"]["text"]
            .as_str()
            .unwrap()
            .contains("Existing reply")
    );
    restore_archived(&mut client, &fixture.cwd, &id).await;
    terminate(&mut process).await;
    let mut process = start_with_path(&state, &log, Some(&fixture.path));
    let address = ready(&mut process, &log).await;
    let mut client = connect(&address, METHODS).await;
    assert_eq!(
        success(
            &mut client,
            "agent.timeline.get.request",
            json!({"agentId":id})
        )
        .await["epoch"],
        epoch
    );
    assert_eq!(
        request(
            &mut client,
            "agent.import.request",
            import(&fixture.cwd, "native")
        )
        .await["code"],
        "invalid_message"
    );
    terminate(&mut process).await;
}

async fn assert_recent_validation(client: &mut Socket, cwd: &Path) {
    for extra in [
        json!({"limit":0}),
        json!({"limit":201}),
        json!({"providers":["unknown"]}),
        json!({"since":"invalid"}),
        json!({"query":"x".repeat(4097)}),
        json!({"cwd":"relative"}),
    ] {
        let mut params = json!({"cwd":cwd});
        params
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        assert_eq!(
            request(client, "provider.sessions.recent.list.request", params).await["code"],
            "invalid_message"
        );
    }
    for extra in [
        json!({"since":"2030-01-01T00:00:00Z"}),
        json!({"providers":[]}),
        json!({"query":"absent"}),
    ] {
        let mut params = json!({"cwd":cwd});
        params
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        assert_eq!(
            success(client, "provider.sessions.recent.list.request", params).await["entries"],
            json!([])
        );
    }
}

async fn restore_archived(client: &mut Socket, cwd: &Path, id: &str) {
    success(client,"agent.update.request",json!({"agentId":id,"name":"User title","labels":{"paseo.parent-agent-id":"old-parent","keep":"yes"}})).await;
    success(client, "agent.archive.request", json!({"agentId":id})).await;
    let recent = success(
        client,
        "provider.sessions.recent.list.request",
        json!({"cwd":cwd}),
    )
    .await;
    assert_eq!(recent["entries"].as_array().unwrap().len(), 1);
    let restored = success(
        client,
        "agent.import.request",
        json!({"provider":"codex","sessionId":"native","cwd":cwd,"labels":{"new":"yes"}}),
    )
    .await;
    assert_eq!(restored["agentId"], id);
    assert_eq!(restored["agent"]["title"], "User title");
    assert_eq!(restored["agent"]["labels"]["keep"], "yes");
    assert_eq!(restored["agent"]["labels"]["new"], "yes");
    assert!(restored["agent"]["labels"]["paseo.parent-agent-id"].is_null());
}

#[tokio::test]
async fn refresh_reconciles_external_history_and_interrupts_owned_execution() {
    let fixture = super::native::NativeFixture::new();
    seed(&fixture.cwd, "native", &json!({}));
    let state = fixture.root.path().join("state");
    let log = fixture.root.path().join("server.log");
    let mut process = start_with_path(&state, &log, Some(&fixture.path));
    let address = ready(&mut process, &log).await;
    let mut client = connect(&address, METHODS).await;
    let created = success(
        &mut client,
        "agent.import.request",
        import(&fixture.cwd, "native"),
    )
    .await;
    let id = created["agentId"].as_str().unwrap();
    let page = success(
        &mut client,
        "agent.timeline.get.request",
        json!({"agentId":id}),
    )
    .await;
    let cursor = page["endCursor"].clone();
    let mut observer = connect(&address, METHODS).await;
    success(
        &mut observer,
        "agent.timeline.set_subscription.request",
        json!({"agentIds":[id]}),
    )
    .await;
    let mut plugin = connect_as(&address, METHODS, "plugin:native-test").await;
    success(&mut plugin,"agent.timeline.append.request",json!({"agentId":id,"item":{"type":"plugin","id":"card","kind":"test","version":1,"data":{"private":"secret"}}})).await;
    assert_eq!(receive(&mut observer).await["method"], "agent_stream");
    let unchanged = success(&mut client, "agent.refresh.request", json!({"agentId":id})).await;
    assert_eq!(unchanged["status"], "agent_refreshed");
    assert_eq!(unchanged["timelineSize"], 3);
    assert_eq!(
        success(
            &mut client,
            "agent.timeline.get.request",
            json!({"agentId":id})
        )
        .await["epoch"],
        page["epoch"]
    );
    history(&fixture.cwd, "native", "Externally rewritten");
    success(&mut client, "agent.refresh.request", json!({"agentId":id})).await;
    let replacement = receive(&mut observer).await;
    assert_eq!(replacement["method"], "agent.timeline.replacement");
    let reset = success(
        &mut client,
        "agent.timeline.get.request",
        json!({"agentId":id,"cursor":cursor}),
    )
    .await;
    assert_eq!(reset["staleCursor"], true);
    assert_eq!(reset["entries"][1]["item"]["text"], "Externally rewritten");
    assert_eq!(reset["entries"][2]["item"]["type"], "plugin");
    assert_eq!(
        request(
            &mut client,
            "agent.fork_context.request",
            json!({"agentId":id,"boundaryCursor":cursor})
        )
        .await["code"],
        "invalid_message"
    );
    let attachment = success(
        &mut client,
        "agent.fork_context.request",
        json!({"agentId":id,"boundaryCursor":reset["endCursor"]}),
    )
    .await;
    assert!(
        !attachment["attachment"]["text"]
            .as_str()
            .unwrap()
            .contains("secret")
    );
    drop(observer);
    assert_refresh_execution(&mut client, &fixture.cwd, id).await;
    terminate(&mut process).await;
}

async fn assert_refresh_execution(client: &mut Socket, cwd: &Path, id: &str) {
    success(
        client,
        "agent.message.send.request",
        json!({"agentId":id,"text":"hang"}),
    )
    .await;
    let refreshed = success(client, "agent.refresh.request", json!({"agentId":id})).await;
    assert_eq!(refreshed["agent"]["status"], "idle", "{refreshed}");
    success(
        client,
        "agent.message.send.request",
        json!({"agentId":id,"text":"after refresh"}),
    )
    .await;
    let finished = success(client, "agent.finish.wait.request", json!({"agentId":id})).await;
    assert_eq!(finished["status"], "idle");
    let requests: Vec<Value> = std::fs::read_to_string(cwd.join("native-requests.jsonl"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert!(
        requests
            .iter()
            .any(|request| request["method"] == "turn/interrupt")
    );
    let resumed: Vec<_> = requests
        .iter()
        .filter(|request| request["method"] == "thread/resume")
        .collect();
    assert_eq!(resumed.len(), 2);
    assert!(
        resumed
            .iter()
            .all(|request| request["params"]["threadId"] == "native")
    );
    assert!(
        requests
            .iter()
            .filter(|request| request["method"] == "turn/start")
            .all(|request| request["params"]["model"] == "import-model")
    );
}

#[tokio::test]
async fn failed_native_imports_leave_no_agents_or_workspaces_and_errors_are_safe() {
    let fixture = super::native::NativeFixture::new();
    seed(&fixture.cwd, "active", &json!({"status":{"type":"active"}}));
    seed(
        &fixture.cwd,
        "wrong-cwd",
        &json!({"cwd":fixture.root.path()}),
    );
    let state = fixture.root.path().join("state");
    let log = fixture.root.path().join("server.log");
    let mut process = start_with_path(&state, &log, Some(&fixture.path));
    let address = ready(&mut process, &log).await;
    let mut client = connect(&address, METHODS).await;
    for id in ["active", "wrong-cwd", "missing-session"] {
        let failed = request(
            &mut client,
            "agent.import.request",
            import(&fixture.cwd, id),
        )
        .await;
        assert_eq!(failed["type"], "error", "{failed}");
        assert!(!failed.to_string().contains("sensitive"));
    }
    for name in [
        "agents/agents.json",
        "projects/workspaces.json",
        "projects/projects.json",
    ] {
        if state.join(name).exists() {
            let value: Value =
                serde_json::from_str(&std::fs::read_to_string(state.join(name)).unwrap()).unwrap();
            assert_eq!(value, json!([]), "{name}: {value}");
        }
    }
    std::fs::write(fixture.cwd.join("behavior"), "error").unwrap();
    let recent = success(
        &mut client,
        "provider.sessions.recent.list.request",
        json!({"cwd":fixture.cwd}),
    )
    .await;
    assert_eq!(recent["entries"], json!([]));
    assert_eq!(recent["providerErrors"][0]["provider"], "codex");
    assert!(!recent.to_string().contains("sensitive"));
    terminate(&mut process).await;
}
