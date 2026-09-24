use std::os::unix::fs::PermissionsExt;

use futures_util::SinkExt;
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::Message;

use super::transport::{connect, receive, request};
use super::{ready, start_with_path, terminate};

const METHODS: &[&str] = &[
    "workspace.open.request",
    "agent.create.request",
    "agent.resume.request",
    "agent.message.send.request",
    "agent.cancel.request",
    "agent.finish.wait.request",
    "agent.get.request",
    "agent.archive.request",
    "agent.delete.request",
];

#[tokio::test]
async fn websocket_executes_native_turns_waits_concurrently_and_resumes_after_restart() {
    let root = tempfile::tempdir().unwrap();
    let native = root.path().join("codex");
    std::fs::write(
        &native,
        include_str!("../../../../crates/server-provider/tests/fixtures/codex_app_server.py"),
    )
    .unwrap();
    std::fs::set_permissions(&native, std::fs::Permissions::from_mode(0o700)).unwrap();
    let cwd = root.path().join("work");
    std::fs::create_dir(&cwd).unwrap();
    let mut paths = vec![root.path().to_path_buf()];
    paths.extend(std::env::split_paths(&std::env::var_os("PATH").unwrap()));
    let path = std::env::join_paths(paths).unwrap();
    let state = root.path().join("state");
    let log = root.path().join("server.log");
    let mut process = start_with_path(&state, &log, Some(&path));
    let address = ready(&mut process, &log).await;
    let mut client = connect(&address, METHODS).await;
    let workspace = request(&mut client, "workspace.open.request", json!({"cwd":cwd})).await;
    assert_eq!(workspace["type"], "response", "{workspace}");
    let created = request(
        &mut client,
        "agent.create.request",
        json!({"config":{"provider":"codex","cwd":cwd,"title":"WS execution"}}),
    )
    .await;
    assert_eq!(created["type"], "response", "{created}");
    let id = created["result"]["agentId"].as_str().unwrap().to_owned();
    let handle = created["result"]["agent"]["persistence"].clone();
    assert_eq!(
        request(
            &mut client,
            "agent.message.send.request",
            json!({"agentId":id,"text":"hello websocket"})
        )
        .await["result"]["accepted"],
        true
    );
    // Accepted work survives the creating connection, and its result is visible on another one.
    client.close(None).await.unwrap();
    let mut client = connect(&address, METHODS).await;
    let finished = request(
        &mut client,
        "agent.finish.wait.request",
        json!({"agentId":id}),
    )
    .await;
    assert_eq!(finished["result"]["lastMessage"], "Echo: hello websocket");
    assert_eq!(finished["result"]["status"], "idle");
    request(
        &mut client,
        "agent.message.send.request",
        json!({"agentId":id,"text":"hang"}),
    )
    .await;
    client.send(Message::Text(json!({"type":"request","request_id":"wait","method":"agent.finish.wait.request","params":{"agentId":id}}).to_string().into())).await.unwrap();
    client.send(Message::Text(json!({"type":"request","request_id":"cancel","method":"agent.cancel.request","params":{"agentId":id}}).to_string().into())).await.unwrap();
    let first = receive(&mut client).await;
    let second = receive(&mut client).await;
    let results = [first, second];
    assert!(
        results
            .iter()
            .any(|result| result["request_id"] == "cancel" && result["type"] == "response")
    );
    assert!(
        results
            .iter()
            .any(|result| result["request_id"] == "wait" && result["result"]["status"] == "idle")
    );
    client = assert_wait_budget(client, &address, &id).await;
    // Drain a still-running turn, then recover precisely the same provider handle and Agent ID.
    request(
        &mut client,
        "agent.message.send.request",
        json!({"agentId":id,"text":"hang"}),
    )
    .await;
    terminate(&mut process).await;
    assert_children_exited(&cwd);
    let mut restarted = start_with_path(&state, &log, Some(&path));
    let address = ready(&mut restarted, &log).await;
    let mut client = connect(&address, METHODS).await;
    assert_restored_lifecycle(&mut client, &id, &handle).await;
    terminate(&mut restarted).await;
    assert_children_exited(&cwd);
    let stored: Value =
        serde_json::from_slice(&std::fs::read(state.join("agents/agents.json")).unwrap()).unwrap();
    assert_eq!(stored, json!([]));
}

async fn assert_wait_budget(
    mut client: super::transport::Socket,
    address: &str,
    id: &str,
) -> super::transport::Socket {
    request(
        &mut client,
        "agent.message.send.request",
        json!({"agentId":id,"text":"hang"}),
    )
    .await;
    for index in 0..33 {
        client
            .send(Message::Text(
                json!({"type":"request","request_id":format!("budget-{index}"),
            "method":"agent.finish.wait.request","params":{"agentId":id}})
                .to_string()
                .into(),
            ))
            .await
            .unwrap();
    }
    let rejected = receive(&mut client).await;
    assert_eq!(rejected["code"], "resource_exhausted", "{rejected}");
    client.close(None).await.unwrap();
    let mut fresh = connect(address, METHODS).await;
    request(&mut fresh, "agent.cancel.request", json!({"agentId":id})).await;
    let waited = request(
        &mut fresh,
        "agent.finish.wait.request",
        json!({"agentId":id}),
    )
    .await;
    assert_eq!(waited["result"]["status"], "idle", "{waited}");
    fresh
}

async fn assert_restored_lifecycle(
    client: &mut super::transport::Socket,
    id: &str,
    handle: &Value,
) {
    let resumed = request(client, "agent.resume.request", json!({"handle":handle})).await;
    assert_eq!(resumed["result"]["agentId"], id, "{resumed}");
    request(
        client,
        "agent.message.send.request",
        json!({"agentId":id,"text":"after restart"}),
    )
    .await;
    assert_eq!(
        request(client, "agent.finish.wait.request", json!({"agentId":id})).await["result"]["lastMessage"],
        "Echo: after restart"
    );
    request(client, "agent.archive.request", json!({"agentId":id})).await;
    let archived = request(client, "agent.resume.request", json!({"handle":handle})).await;
    assert!(!archived["result"]["agent"]["archivedAt"].is_null());
    assert_eq!(
        request(
            client,
            "agent.message.send.request",
            json!({"agentId":id,"text":"rejected"})
        )
        .await["result"]["accepted"],
        false
    );
    request(client, "agent.delete.request", json!({"agentId":id})).await;
}

fn assert_children_exited(cwd: &std::path::Path) {
    let records = std::fs::read_to_string(cwd.join("native-requests.jsonl")).unwrap();
    let pids = records
        .lines()
        .map(|line| {
            serde_json::from_str::<Value>(line).unwrap()["pid"]
                .as_u64()
                .unwrap()
        })
        .collect::<std::collections::BTreeSet<_>>();
    for pid in pids {
        assert!(
            !std::process::Command::new("kill")
                .args(["-0", &pid.to_string()])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .unwrap()
                .success(),
            "native child still alive: {pid}"
        );
    }
}
