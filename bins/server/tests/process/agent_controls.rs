use serde_json::{Value, json};

use super::transport::{Socket, connect, receive, request};
use super::{ready, start_with_path, terminate};

const METHODS: &[&str] = &[
    "workspace.open.request",
    "agent.create.request",
    "agent.get.request",
    "agent.message.send.request",
    "agent.finish.wait.request",
    "agent.timeline.get.request",
    "agent.timeline.set_subscription.request",
    "agent.rewind.request",
    "agent.commands.list.request",
    "agent.mode.set.request",
    "agent.feature.set.request",
    "agent.permission.resolve.request",
    "agent.provider_subagents.list.request",
    "agent.provider_subagents.timeline.get.request",
    "provider.diagnostic.request",
    "provider.usage.list.request",
];

async fn success(client: &mut Socket, method: &str, params: Value) -> Value {
    let response = request(client, method, params).await;
    assert_eq!(response["type"], "response", "{method}: {response}");
    response["result"].clone()
}

#[tokio::test]
async fn controls_route_over_websocket_and_pending_approvals_survive_client_reconnect() {
    let fixture = super::native::NativeFixture::new();
    let state = fixture.root.path().join("state");
    let log = fixture.root.path().join("server.log");
    let mut process = start_with_path(&state, &log, Some(&fixture.path));
    let address = ready(&mut process, &log).await;
    let mut client = connect(&address, METHODS).await;
    success(
        &mut client,
        "workspace.open.request",
        json!({"cwd":fixture.cwd}),
    )
    .await;
    let created = success(
        &mut client,
        "agent.create.request",
        json!({"config":{"provider":"codex","cwd":fixture.cwd}}),
    )
    .await;
    let id = created["agentId"].as_str().unwrap();
    inspect_and_configure(&mut client, id).await;
    let mut observer = connect(&address, METHODS).await;
    success(
        &mut observer,
        "agent.timeline.set_subscription.request",
        json!({"agentIds":[id]}),
    )
    .await;
    success(
        &mut client,
        "agent.message.send.request",
        json!({"agentId":id,"text":"permit-command"}),
    )
    .await;
    let permission = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let event = receive(&mut observer).await;
            if event["method"] == "agent_stream"
                && event["params"]["event"]["type"] == "permission_requested"
            {
                break event["params"]["event"]["request"].clone();
            }
        }
    })
    .await
    .unwrap();
    drop(client);
    let mut client = connect(&address, METHODS).await;
    let snapshot = success(&mut client, "agent.get.request", json!({"agentId":id})).await;
    assert_eq!(
        snapshot["agent"]["pendingPermissions"][0]["id"],
        permission["id"]
    );
    success(
        &mut client,
        "agent.permission.resolve.request",
        json!({"agentId":id,"requestId":permission["id"],"response":{"behavior":"allow"}}),
    )
    .await;
    let resolved = receive(&mut observer).await;
    assert_eq!(resolved["params"]["event"]["type"], "permission_resolved");
    success(
        &mut client,
        "agent.finish.wait.request",
        json!({"agentId":id}),
    )
    .await;
    let page = success(
        &mut client,
        "agent.timeline.get.request",
        json!({"agentId":id}),
    )
    .await;
    assert_eq!(success(&mut client,"agent.rewind.request",json!({"agentId":id,"messageId":page["entries"][0]["item"]["messageId"],"mode":"conversation"})).await["ok"],true);
    let children = success(
        &mut client,
        "agent.provider_subagents.list.request",
        json!({"parentAgentId":id}),
    )
    .await;
    assert_eq!(children["subagents"], json!([]));
    assert_eq!(
        request(
            &mut client,
            "agent.provider_subagents.timeline.get.request",
            json!({"parentAgentId":id,"subagentId":"unrelated"})
        )
        .await["code"],
        "agent_not_found"
    );
    terminate(&mut process).await;
}

async fn inspect_and_configure(client: &mut Socket, id: &str) {
    let diagnostic = success(
        client,
        "provider.diagnostic.request",
        json!({"provider":"codex"}),
    )
    .await;
    assert!(
        diagnostic["diagnostic"]
            .as_str()
            .unwrap()
            .contains("ChatGPT login")
    );
    assert!(!diagnostic.to_string().contains("private@example.test"));
    let usage = success(client, "provider.usage.list.request", json!({})).await;
    assert_eq!(
        usage["providers"]
            .as_array()
            .unwrap()
            .iter()
            .find(|provider| provider["providerId"] == "codex")
            .unwrap()["status"],
        "available"
    );
    let commands = success(client, "agent.commands.list.request", json!({"agentId":id})).await;
    assert!(
        commands["commands"]
            .as_array()
            .unwrap()
            .iter()
            .any(|command| command["kind"] == "skill")
    );
    assert_eq!(
        success(
            client,
            "agent.mode.set.request",
            json!({"agentId":id,"modeId":"auto"})
        )
        .await["accepted"],
        true
    );
    assert_eq!(
        success(
            client,
            "agent.feature.set.request",
            json!({"agentId":id,"featureId":"fast_mode","value":true})
        )
        .await["accepted"],
        true
    );
}
