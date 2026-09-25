use std::path::Path;

use futures_util::SinkExt;
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::Message;

use super::transport::{Socket, connect, receive, request};
use super::{ready, start, terminate};

const METHODS: &[&str] = &[
    "push.register",
    "push.unregister.request",
    "session.heartbeat",
    "connection.ping",
];

async fn event(socket: &mut Socket, method: &str, params: Value) {
    socket
        .send(Message::Text(
            json!({"type":"event","method":method,"params":params})
                .to_string()
                .into(),
        ))
        .await
        .unwrap();
}

async fn barrier(socket: &mut Socket) {
    let reply = request(socket, "connection.ping", json!({"nonce":"persisted"})).await;
    assert_eq!(reply["result"]["nonce"], "persisted", "{reply}");
}

fn tokens(path: &Path) -> Vec<String> {
    let value: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    value["subscriptions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["token"].as_str().unwrap().to_owned())
        .collect()
}

fn heartbeat() -> Value {
    json!({"deviceType":"mobile","focusedAgentId":null,"lastActivityAt":chrono::Utc::now().to_rfc3339(),"appVisible":true})
}

#[tokio::test]
async fn push_registration_is_durable_and_heartbeat_ownership_is_connection_local() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("state");
    let path = directory.join("push-tokens.json");
    let log = root.path().join("server.log");
    let mut process = start(&directory, &log);
    let address = ready(&mut process, &log).await;
    let mut first = connect(&address, METHODS).await;
    let mut second = connect(&address, METHODS).await;
    event(
        &mut first,
        "push.register",
        json!({"token":" private-token "}),
    )
    .await;
    barrier(&mut first).await;
    assert_eq!(tokens(&path), ["private-token"]);
    // Persistence failure must leave this connection's token registered for a retry.
    let saved = std::fs::read(&path).unwrap();
    std::fs::remove_file(&path).unwrap();
    std::fs::create_dir(&path).unwrap();
    let failure = request(
        &mut first,
        "push.unregister.request",
        json!({"token":"private-token"}),
    )
    .await;
    assert_eq!(failure["type"], "error");
    assert!(!failure.to_string().contains("private-token"));
    std::fs::remove_dir(&path).unwrap();
    std::fs::write(&path, saved).unwrap();
    let leases: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    let expiry = chrono::DateTime::parse_from_rfc3339(
        leases["subscriptions"][0]["expiresAt"].as_str().unwrap(),
    )
    .unwrap();
    assert!(
        (expiry.timestamp_millis() - chrono::Utc::now().timestamp_millis() - 48 * 60 * 60 * 1000)
            .abs()
            < 10_000
    );
    let revoked = request(
        &mut second,
        "push.unregister.request",
        json!({"token":"private-token"}),
    )
    .await;
    assert_eq!(revoked["result"], json!({}));
    assert!(tokens(&path).is_empty());
    // A different connection retains its registration, matching Paseo's source-local metadata.
    event(&mut first, "session.heartbeat", heartbeat()).await;
    barrier(&mut first).await;
    assert_eq!(tokens(&path), ["private-token"]);
    request(
        &mut first,
        "push.unregister.request",
        json!({"token":"private-token"}),
    )
    .await;
    event(&mut first, "session.heartbeat", heartbeat()).await;
    barrier(&mut first).await;
    assert!(tokens(&path).is_empty());
    event(
        &mut first,
        "push.register",
        json!({"token":"survives-restart"}),
    )
    .await;
    barrier(&mut first).await;
    drop(first);
    drop(second);
    terminate(&mut process).await;
    let mut process = start(&directory, &log);
    let address = ready(&mut process, &log).await;
    let mut socket = connect(&address, METHODS).await;
    assert_eq!(tokens(&path), ["survives-restart"]);
    let bad = request(&mut socket, "push.unregister.request", json!({"token":7})).await;
    assert_eq!(bad["code"], "invalid_message");
    event(&mut socket, "push.register", json!({"token":7})).await;
    assert_eq!(receive(&mut socket).await["code"], "invalid_message");
    request(
        &mut socket,
        "push.unregister.request",
        json!({"token":"survives-restart"}),
    )
    .await;
    assert!(tokens(&path).is_empty());
    assert!(
        !std::fs::read_to_string(log)
            .unwrap()
            .contains("survives-restart")
    );
    drop(socket);
    terminate(&mut process).await;
}
