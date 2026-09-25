use futures_util::SinkExt;
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::Message;

use super::native::NativeFixture;
use super::transport::{Socket, connect, receive, request};
use super::{ready, start, start_with_path, terminate};

const SESSION_METHODS: &[&str] = &[
    "session.events.set_subscription.request",
    "subscription.release.request",
    "session.heartbeat",
    "connection.ping",
];

async fn heartbeat(socket: &mut Socket, focused: Option<&str>) {
    socket
        .send(Message::Text(
            json!({"type":"event","method":"session.heartbeat","params":{
                "deviceType":"web","focusedAgentId":focused,"appVisible":true,
                "lastActivityAt":chrono::Utc::now().to_rfc3339(),
            }})
            .to_string()
            .into(),
        ))
        .await
        .unwrap();
    // Heartbeat has no response. The subsequent ping is a barrier on the same connection.
    assert_eq!(
        request(socket, "connection.ping", json!({"nonce":"barrier"})).await["result"]["nonce"],
        "barrier"
    );
}

async fn subscribe(socket: &mut Socket, events: &[&str]) -> String {
    let result = request(
        socket,
        "session.events.set_subscription.request",
        json!({"events":events,"notifications":true}),
    )
    .await;
    assert_eq!(result["type"], "response", "{result}");
    result["result"]["subscriptionId"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[tokio::test]
async fn config_and_attention_streams_are_connection_owned_and_presence_controls_notifications() {
    let fixture = NativeFixture::new();
    let state = fixture.root.path().join("state");
    let log = fixture.root.path().join("server.log");
    let mut process = start_with_path(&state, &log, Some(&fixture.path));
    let address = ready(&mut process, &log).await;
    let mut observer = connect(&address, SESSION_METHODS).await;
    let subscription = subscribe(&mut observer, &["agent_attention_required"]).await;
    heartbeat(&mut observer, None).await;
    let mut writer = connect(
        &address,
        &[
            "workspace.open.request",
            "agent.create.request",
            "agent.message.send.request",
            "agent.finish.wait.request",
            "agent.model.set.request",
            "agent.thinking.set.request",
            "agent.config.apply.request",
            "agent.get.request",
            "subscription.release.request",
        ],
    )
    .await;
    request(
        &mut writer,
        "workspace.open.request",
        json!({"cwd":fixture.cwd}),
    )
    .await;
    let created = request(
        &mut writer,
        "agent.create.request",
        json!({"config":{"provider":"codex","cwd":fixture.cwd}}),
    )
    .await;
    let id = created["result"]["agentId"].as_str().unwrap();
    assert_eq!(
        request(
            &mut writer,
            "agent.model.set.request",
            json!({"agentId":id,"modelId":"test-model"})
        )
        .await["result"]["accepted"],
        true
    );
    assert_eq!(
        request(
            &mut writer,
            "agent.thinking.set.request",
            json!({"agentId":id,"thinkingOptionId":"high"})
        )
        .await["result"]["accepted"],
        true
    );
    assert_attention_delivery(&mut writer, &mut observer, id, &subscription, &state).await;
    assert_eq!(
        request(
            &mut writer,
            "agent.config.apply.request",
            json!({"agentId":id,"config":{"modelId":null,"thinkingOptionId":null}})
        )
        .await["result"]["accepted"],
        true
    );
    request(
        &mut observer,
        "subscription.release.request",
        json!({"subscriptionId":subscription}),
    )
    .await;
    request(
        &mut writer,
        "agent.message.send.request",
        json!({"agentId":id,"text":"released"}),
    )
    .await;
    request(
        &mut writer,
        "agent.finish.wait.request",
        json!({"agentId":id}),
    )
    .await;
    assert_eq!(
        request(
            &mut observer,
            "connection.ping",
            json!({"nonce":"no-event"})
        )
        .await["result"]["nonce"],
        "no-event"
    );
    terminate(&mut process).await;
}

#[tokio::test]
async fn event_subscriptions_enforce_budgets_validate_heartbeats_and_publish_config_and_drain() {
    let root = tempfile::tempdir().unwrap();
    let log = root.path().join("server.log");
    let mut process = start(&root.path().join("state"), &log);
    let address = ready(&mut process, &log).await;
    let mut observer = connect(&address, SESSION_METHODS).await;
    assert_eq!(
        request(
            &mut observer,
            "session.events.set_subscription.request",
            json!({"events":["activity_log"]})
        )
        .await["code"],
        "unsupported_capability"
    );
    observer.send(Message::Text(json!({"type":"event","method":"session.heartbeat","params":{
        "deviceType":"web","focusedAgentId":null,"appVisible":true,"lastActivityAt":"invalid",
    }}).to_string().into())).await.unwrap();
    assert_eq!(receive(&mut observer).await["code"], "invalid_message");
    assert_eq!(
        request(&mut observer, "session.heartbeat", json!({})).await["code"],
        "invalid_message"
    );
    let mut ids = Vec::new();
    for _ in 0..16 {
        ids.push(subscribe(&mut observer, &[]).await);
    }
    assert_eq!(
        request(
            &mut observer,
            "session.events.set_subscription.request",
            json!({"events":[]})
        )
        .await["code"],
        "resource_exhausted"
    );
    for id in ids {
        request(
            &mut observer,
            "subscription.release.request",
            json!({"subscriptionId":id}),
        )
        .await;
    }
    let id = subscribe(
        &mut observer,
        &["status.daemon_config_changed", "status.server_info"],
    )
    .await;
    let mut writer = connect(
        &address,
        &["daemon.config.set.request", "daemon.config.reload.request"],
    )
    .await;
    for (method, params) in [
        (
            "daemon.config.set.request",
            json!({"config":{"browserTools":{"enabled":true}}}),
        ),
        ("daemon.config.reload.request", json!({})),
    ] {
        assert_eq!(
            request(&mut writer, method, params).await["type"],
            "response"
        );
        let event = receive(&mut observer).await;
        assert_eq!(event["method"], "status.daemon_config_changed");
        assert_eq!(event["params"]["subscriptionId"], id);
        assert_eq!(event["params"]["config"]["browserTools"]["enabled"], true);
    }
    let mut unnegotiated = connect(&address, &[]).await;
    unnegotiated
        .send(Message::Text(
            json!({"type":"event","method":"session.heartbeat","params":{}})
                .to_string()
                .into(),
        ))
        .await
        .unwrap();
    assert_eq!(
        receive(&mut unnegotiated).await["code"],
        "unsupported_capability"
    );
    terminate(&mut process).await;
    let event = receive(&mut observer).await;
    assert_eq!(event["method"], "status.server_info");
    assert_eq!(event["params"]["info"]["lifecycle"], "draining");
}

async fn assert_attention_delivery(
    writer: &mut Socket,
    observer: &mut Socket,
    id: &str,
    subscription: &str,
    state: &std::path::Path,
) {
    for (prompt, expected) in [("one", true), ("two", false), ("fail", true)] {
        heartbeat(observer, (prompt == "two").then_some(id)).await;
        request(
            writer,
            "agent.message.send.request",
            json!({"agentId":id,"text":prompt}),
        )
        .await;
        let event = receive(observer).await;
        assert_eq!(event["method"], "agent_attention_required");
        assert_eq!(event["params"]["subscriptionId"], subscription);
        assert_eq!(event["params"]["agentId"], id);
        assert_eq!(event["params"]["shouldNotify"], expected);
        assert_eq!(
            event["params"]["reason"],
            if prompt == "fail" {
                "error"
            } else {
                "finished"
            }
        );
        let stored: Value =
            serde_json::from_slice(&std::fs::read(state.join("agents/agents.json")).unwrap())
                .unwrap();
        assert_ne!(stored[0]["lastStatus"], "running");
        let latest = request(writer, "agent.get.request", json!({"agentId":id})).await;
        assert_eq!(latest["result"]["agent"]["model"], "test-model");
        // Another connection cannot release this observer.
        request(
            writer,
            "subscription.release.request",
            json!({"subscriptionId":subscription}),
        )
        .await;
    }
}
