use serde_json::{Value, json};

use super::transport::{Socket, connect, receive, request};
use super::{ready, start_with_path, terminate};

const METHODS: &[&str] = &[
    "workspace.open.request",
    "agent.create.request",
    "agent.message.send.request",
    "agent.finish.wait.request",
    "agent.timeline.get.request",
    "agent.timeline.set_subscription.request",
    "agent.refresh.request",
];

async fn success(client: &mut Socket, method: &str, params: Value) -> Value {
    let response = request(client, method, params).await;
    assert_eq!(response["type"], "response", "{response}");
    response["result"].clone()
}

#[tokio::test]
async fn native_streams_replay_on_reconnect_and_steer_completes_without_duplicate_text() {
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
    let mut observer = connect(&address, METHODS).await;
    success(
        &mut observer,
        "agent.timeline.set_subscription.request",
        json!({"agentIds":[id]}),
    )
    .await;
    assert_eq!(
        success(
            &mut client,
            "agent.message.send.request",
            json!({"agentId":id,"text":"stream"})
        )
        .await["accepted"],
        true
    );
    let prefix = receive_prefix(&mut observer).await;
    assert_eq!(
        success(
            &mut client,
            "agent.finish.wait.request",
            json!({"agentId":id,"timeoutMs":1})
        )
        .await["status"],
        "timeout"
    );
    drop(observer);
    let page = success(
        &mut client,
        "agent.timeline.get.request",
        json!({"agentId":id,"limit":0}),
    )
    .await;
    assert_eq!(page["endCursor"], prefix);
    let mut observer = connect(&address, METHODS).await;
    success(
        &mut observer,
        "agent.timeline.set_subscription.request",
        json!({"agentIds":[id]}),
    )
    .await;
    assert_eq!(
        success(
            &mut client,
            "agent.message.send.request",
            json!({"agentId":id,"text":"continue","activeTurnBehavior":"steer"})
        )
        .await["accepted"],
        true
    );
    let suffix = receive_completion(&mut observer).await;
    assert_eq!(suffix, "stream + continue");
    let catch_up = success(
        &mut client,
        "agent.timeline.get.request",
        json!({"agentId":id,"cursor":prefix,"direction":"after"}),
    )
    .await;
    assert_eq!(catch_up["entries"].as_array().unwrap().len(), 2);
    assert_eq!(catch_up["entries"][1]["item"]["text"], suffix);
    let before = success(
        &mut client,
        "agent.timeline.get.request",
        json!({"agentId":id,"limit":0}),
    )
    .await;
    drop(observer);
    drop(client);
    terminate(&mut process).await;
    let mut restarted = start_with_path(&state, &log, Some(&fixture.path));
    let address = ready(&mut restarted, &log).await;
    let mut client = connect(&address, METHODS).await;
    let after = success(
        &mut client,
        "agent.timeline.get.request",
        json!({"agentId":id,"limit":0}),
    )
    .await;
    assert_eq!(after["epoch"], before["epoch"]);
    assert_eq!(after["entries"], before["entries"]);
    terminate(&mut restarted).await;
}

async fn receive_prefix(observer: &mut Socket) -> Value {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        let mut thought = String::new();
        let mut output_seen = false;
        let mut previous = 0;
        loop {
            let event = receive(observer).await;
            let payload = &event["params"];
            let item = &payload["event"]["item"];
            let seq = payload["seq"].as_u64().unwrap();
            assert_eq!(seq, previous + 1);
            previous = seq;
            if item["type"] == "reasoning" {
                thought.push_str(item["text"].as_str().unwrap());
            }
            output_seen |= item["detail"]["output"] == "offline output";
            if item["type"] == "assistant_message" {
                assert_eq!(item["text"], "Echo: ");
                assert_eq!(thought, "Thinking");
                assert!(output_seen);
                break json!({"epoch":payload["epoch"],"seq":seq});
            }
        }
    })
    .await
    .unwrap()
}

async fn receive_completion(observer: &mut Socket) -> String {
    let mut suffix = String::new();
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let event = receive(observer).await;
            let payload = &event["params"];
            if payload["event"]["type"] == "turn_completed" {
                break;
            }
            if payload["event"]["item"]["type"] == "assistant_message" {
                suffix.push_str(payload["event"]["item"]["text"].as_str().unwrap());
            }
        }
    })
    .await
    .unwrap();
    suffix
}
