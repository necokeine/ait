//! Paseo timeline cursor and observer contracts over the production Codex fixture host.

use serde_json::Value;

use super::super::transport::Socket;
use super::*;

struct Fixture {
    process: super::super::Process,
    client: Socket,
    address: String,
    agent: Value,
    _native: super::super::native::NativeFixture,
}

impl Fixture {
    async fn start() -> Self {
        let native = super::super::native::NativeFixture::new();
        let log = native.root.path().join("server.log");
        let mut process =
            start_with_path(&native.root.path().join("state"), &log, Some(&native.path));
        let address = ready(&mut process, &log).await;
        let mut client = connect(&address, METHODS).await;
        request(
            &mut client,
            "workspace.open.request",
            json!({"cwd":native.cwd}),
        )
        .await;
        let created = request(
            &mut client,
            "agent.create.request",
            json!({"config":{"provider":"codex","cwd":native.cwd},"initialPrompt":"timeline test"}),
        )
        .await;
        assert_eq!(created["type"], "response", "{created}");
        let agent = created["result"]["agentId"].clone();
        let completed = request(
            &mut client,
            "agent.finish.wait.request",
            json!({"agentId":agent}),
        )
        .await;
        assert_eq!(completed["result"]["status"], "idle");
        Self {
            process,
            client,
            address,
            agent,
            _native: native,
        }
    }

    async fn peer(&self, identity: &str) -> Socket {
        let mut methods = METHODS.to_vec();
        methods.push("connection.ping");
        connect_as(&self.address, &methods, identity).await
    }

    async fn stop(mut self) {
        self.client.close(None).await.unwrap();
        terminate(&mut self.process).await;
    }
}

#[tokio::test]
async fn oversized_before_cursor_clamps_to_tail_while_incremental_gap_requests_reset() {
    let mut fixture = Fixture::start().await;
    let page = request(
        &mut fixture.client,
        "agent.timeline.get.request",
        json!({"agentId":fixture.agent}),
    )
    .await;
    let epoch = page["result"]["epoch"].clone();
    let newest = page["result"]["window"]["maxSeq"].as_u64().unwrap();
    assert_eq!(newest, 2);
    let before = request(&mut fixture.client, "agent.timeline.get.request", json!({"agentId":fixture.agent,"cursor":{"epoch":epoch,"seq":newest+100},"direction":"before","limit":1})).await;
    assert_eq!(before["result"]["reset"], false);
    assert_eq!(before["result"]["gap"], false);
    assert_eq!(before["result"]["entries"].as_array().unwrap().len(), 1);
    assert_eq!(before["result"]["endCursor"]["seq"], newest);
    let after = request(&mut fixture.client, "agent.timeline.get.request", json!({"agentId":fixture.agent,"cursor":{"epoch":epoch,"seq":newest+100},"direction":"after","limit":1})).await;
    assert_eq!(after["result"]["reset"], true);
    assert_eq!(after["result"]["gap"], true);
    assert_eq!(after["result"]["staleCursor"], false);
    assert_eq!(after["result"]["endCursor"]["seq"], newest);
    fixture.stop().await;
}

async fn append(socket: &mut Socket, agent: &Value, id: &str) {
    let response = request(socket, "agent.timeline.append.request", json!({"agentId":agent,"item":{"type":"plugin","id":id,"kind":"test","version":1,"data":{"text":id}}})).await;
    assert_eq!(response["type"], "response", "{response}");
}

#[tokio::test]
async fn duplicate_timeline_observers_do_not_share_teardown_or_broadcast_to_idle_peers() {
    let fixture = Fixture::start().await;
    let mut owner = fixture.peer("same-client").await;
    let mut idle = fixture.peer("same-client").await;
    let mut plugin = fixture.peer("plugin:test-writer").await;
    let first = request(
        &mut owner,
        "agent.timeline.set_subscription.request",
        json!({"agentIds":[fixture.agent]}),
    )
    .await;
    let second = request(
        &mut owner,
        "agent.timeline.set_subscription.request",
        json!({"agentIds":[fixture.agent]}),
    )
    .await;
    assert_ne!(
        first["result"]["subscriptionId"],
        second["result"]["subscriptionId"]
    );
    append(&mut plugin, &fixture.agent, "first-card").await;
    let both = [receive(&mut owner).await, receive(&mut owner).await];
    for id in [
        &first["result"]["subscriptionId"],
        &second["result"]["subscriptionId"],
    ] {
        assert!(
            both.iter().any(|event| event["method"] == "agent_stream"
                && event["params"]["subscriptionId"] == *id)
        );
    }
    request(
        &mut idle,
        "subscription.release.request",
        json!({"subscriptionId":second["result"]["subscriptionId"]}),
    )
    .await;
    request(
        &mut owner,
        "subscription.release.request",
        json!({"subscriptionId":first["result"]["subscriptionId"]}),
    )
    .await;
    append(&mut plugin, &fixture.agent, "second-card").await;
    let event = receive(&mut owner).await;
    assert_eq!(
        event["params"]["subscriptionId"],
        second["result"]["subscriptionId"]
    );
    assert_eq!(
        event["params"]["event"]["item"]["data"]["text"],
        "second-card"
    );
    for socket in [&mut owner, &mut idle] {
        assert_eq!(
            request(socket, "connection.ping", json!({"nonce":"source-fence"})).await["result"]["nonce"],
            "source-fence"
        );
    }
    fixture.stop().await;
}
