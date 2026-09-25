use super::transport::{connect, connect_as, receive, request};
use super::{ready, start_with_path, terminate};
use serde_json::json;

const METHODS: &[&str] = &[
    "workspace.open.request",
    "workspace.create.request",
    "agent.create.request",
    "agent.get.request",
    "agent.message.send.request",
    "agent.finish.wait.request",
    "agent.timeline.get.request",
    "agent.timeline.search.request",
    "agent.timeline.list_prompts.request",
    "agent.timeline.append.request",
    "agent.timeline.set_subscription.request",
    "provider.available.list.request",
    "provider.models.list.request",
    "provider.modes.list.request",
    "provider.features.list.request",
    "provider.snapshot.get.request",
    "provider.snapshot.refresh.request",
    "session.events.set_subscription.request",
    "creation.subscribe.request",
    "subscription.release.request",
];

#[tokio::test]
async fn durable_timeline_discovery_creation_and_connection_lifetimes_work_over_websocket() {
    let fixture = super::native::NativeFixture::new();
    let state = fixture.root.path().join("state");
    let log = fixture.root.path().join("server.log");
    let mut process = start_with_path(&state, &log, Some(&fixture.path));
    let address = ready(&mut process, &log).await;
    let mut client = connect(&address, METHODS).await;
    assert_creation_rejects_non_objects(&mut client).await;
    assert_discovery(&mut client, &fixture.cwd).await;
    let workspace = request(
        &mut client,
        "workspace.open.request",
        json!({"cwd":fixture.cwd}),
    )
    .await;
    assert_eq!(workspace["type"], "response");
    let mut observer = connect(&address, METHODS).await;
    let watched = request(
        &mut observer,
        "creation.subscribe.request",
        json!({"kind":"agent","idempotencyKey":"create-one"}),
    )
    .await;
    assert!(watched["result"]["snapshot"].is_null());
    let intent = json!({"config":{"provider":"codex","cwd":fixture.cwd},"idempotencyKey":"create-one","initialPrompt":"first history","subscribe":true});
    let created = request(&mut client, "agent.create.request", intent.clone()).await;
    assert_eq!(created["type"], "response", "{created}");
    assert_eq!(created["result"]["creation"]["phase"], "completed");
    let id = created["result"]["agentId"].as_str().unwrap().to_owned();
    for phase in ["accepted", "agent_ready", "prompt_started", "completed"] {
        let event = receive(&mut observer).await;
        assert_eq!(event["method"], "agent.create.update");
        assert_eq!(event["params"]["phase"], phase);
        let inline = receive(&mut client).await;
        assert_eq!(inline["method"], "agent.create.update");
        assert_eq!(inline["params"]["phase"], phase);
        assert_eq!(
            inline["params"]["subscriptionId"],
            created["result"]["subscriptionId"]
        );
    }
    request(
        &mut client,
        "subscription.release.request",
        json!({"subscriptionId":created["result"]["subscriptionId"]}),
    )
    .await;
    let replay = request(&mut client, "agent.create.request", intent.clone()).await;
    assert_eq!(replay["result"]["agentId"], id);
    let mut conflict = intent.clone();
    conflict["initialPrompt"] = json!("different");
    assert_eq!(
        request(&mut client, "agent.create.request", conflict).await["code"],
        "idempotency_conflict"
    );
    let finished = request(
        &mut client,
        "agent.finish.wait.request",
        json!({"agentId":id}),
    )
    .await;
    assert_eq!(finished["result"]["status"], "idle", "{finished}");
    let page = request(
        &mut client,
        "agent.timeline.get.request",
        json!({"agentId":id}),
    )
    .await;
    assert_eq!(
        page["result"]["entries"].as_array().unwrap().len(),
        2,
        "{page}"
    );
    let epoch = page["result"]["epoch"].clone();
    assert_eq!(
        page["result"]["entries"][0]["item"]["text"],
        "first history"
    );
    let search = request(
        &mut client,
        "agent.timeline.search.request",
        json!({"agentId":id,"query":"FIRST   history"}),
    )
    .await;
    assert_eq!(search["result"]["locations"].as_array().unwrap().len(), 2);
    let prompts = request(
        &mut client,
        "agent.timeline.list_prompts.request",
        json!({"agentId":id}),
    )
    .await;
    assert_eq!(prompts["result"]["prompts"].as_array().unwrap().len(), 1);
    assert_plugin_append_and_release(&address, &id, &mut client).await;
    assert_workspace_receipt(&mut client, &fixture.cwd).await;
    terminate(&mut process).await;
    let mut process = start_with_path(&state, &log, Some(&fixture.path));
    let address = ready(&mut process, &log).await;
    let mut client = connect(&address, METHODS).await;
    assert_restored(&mut client, &id, intent, epoch).await;
    terminate(&mut process).await;
}

async fn assert_creation_rejects_non_objects(client: &mut super::transport::Socket) {
    for method in ["agent.create.request", "workspace.create.request"] {
        for params in [json!(null), json!([]), json!("invalid"), json!(42)] {
            let rejected = request(client, method, params).await;
            assert_eq!(rejected["code"], "invalid_message", "{rejected}");
        }
    }
}

async fn assert_discovery(client: &mut super::transport::Socket, cwd: &std::path::Path) {
    let available = request(client, "provider.available.list.request", json!({})).await;
    assert_eq!(
        available["result"]["providers"][0]["available"], true,
        "{available}"
    );
    for (method, field) in [
        ("provider.models.list.request", "models"),
        ("provider.modes.list.request", "modes"),
        ("provider.features.list.request", "features"),
    ] {
        let result = request(client, method, json!({"provider":"codex","cwd":cwd})).await;
        assert!(result["result"][field].is_array(), "{result}");
    }
    let snapshot = request(client, "provider.snapshot.get.request", json!({"cwd":cwd})).await;
    let conditional = request(
        client,
        "provider.snapshot.get.request",
        json!({"cwd":cwd,"ifNoneMatch":snapshot["result"]["snapshotHash"]}),
    )
    .await;
    assert_eq!(conditional["result"]["notModified"], true);
    let subscribed = request(
        client,
        "session.events.set_subscription.request",
        json!({"events":["providers_snapshot_update"]}),
    )
    .await;
    assert_eq!(subscribed["type"], "response");
    let first = request(
        client,
        "provider.snapshot.refresh.request",
        json!({"cwd":cwd}),
    )
    .await;
    let second = receive(client).await;
    let pair = [first, second];
    assert!(
        pair.iter()
            .any(|value| value["result"]["acknowledged"] == true)
    );
    assert!(
        pair.iter()
            .any(|value| value["method"] == "providers_snapshot_update")
    );
    request(
        client,
        "subscription.release.request",
        json!({"subscriptionId":subscribed["result"]["subscriptionId"]}),
    )
    .await;
}

async fn assert_plugin_append_and_release(
    address: &str,
    id: &str,
    client: &mut super::transport::Socket,
) {
    let append = json!({"agentId":id,"item":{"type":"plugin","id":"card","kind":"test","version":1,"data":{"text":"card"}}});
    assert_eq!(
        request(client, "agent.timeline.append.request", append.clone()).await["code"],
        "unsupported_capability"
    );
    let mut plugin = connect_as(address, METHODS, "plugin:fixture").await;
    let subscription = request(
        client,
        "agent.timeline.set_subscription.request",
        json!({"agentIds":[id]}),
    )
    .await;
    assert_eq!(subscription["type"], "response", "{subscription}");
    let result = request(&mut plugin, "agent.timeline.append.request", append.clone()).await;
    assert_eq!(result["result"]["seq"], 3, "{result}");
    let event = receive(client).await;
    assert_eq!(event["method"], "agent_stream");
    assert_eq!(event["params"]["event"]["item"]["pluginId"], "fixture");
    assert_eq!(
        request(&mut plugin, "agent.timeline.append.request", append.clone()).await["result"]["seq"],
        3
    );
    let mut changed = append;
    changed["item"]["data"] = json!({"changed":true});
    assert_eq!(
        request(&mut plugin, "agent.timeline.append.request", changed).await["code"],
        "idempotency_conflict"
    );
    let released = request(
        client,
        "subscription.release.request",
        json!({"subscriptionId":subscription["result"]["subscriptionId"]}),
    )
    .await;
    assert_eq!(released["type"], "response");
}

async fn assert_workspace_receipt(client: &mut super::transport::Socket, cwd: &std::path::Path) {
    let intent = json!({"source":{"kind":"directory","path":cwd},"idempotencyKey":"workspace-one","subscribe":true});
    let created = request(client, "workspace.create.request", intent).await;
    assert_eq!(created["type"], "response", "{created}");
    for phase in ["accepted", "workspace_ready", "completed"] {
        let event = receive(client).await;
        assert_eq!(event["params"]["phase"], phase);
    }
    request(
        client,
        "subscription.release.request",
        json!({"subscriptionId":created["result"]["subscriptionId"]}),
    )
    .await;
    let snapshot = request(
        client,
        "creation.subscribe.request",
        json!({"kind":"workspace","idempotencyKey":"workspace-one","subscribe":false}),
    )
    .await;
    assert_eq!(
        snapshot["result"]["snapshot"]["workspaceId"],
        created["result"]["workspace"]["id"]
    );
}

async fn assert_restored(
    client: &mut super::transport::Socket,
    id: &str,
    intent: serde_json::Value,
    epoch: serde_json::Value,
) {
    let restored = request(client, "agent.timeline.get.request", json!({"agentId":id})).await;
    assert_eq!(restored["result"]["epoch"], epoch, "{restored}");
    assert_eq!(restored["result"]["entries"].as_array().unwrap().len(), 3);
    let receipt = request(
        client,
        "creation.subscribe.request",
        json!({"kind":"agent","idempotencyKey":"create-one","subscribe":false}),
    )
    .await;
    assert_eq!(receipt["result"]["snapshot"]["agentId"], id);
    assert_eq!(receipt["result"]["snapshot"]["phase"], "completed");
    assert_eq!(
        request(client, "agent.create.request", intent).await["result"]["agentId"],
        id
    );
}
