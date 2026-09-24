use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use server_terminal::protocol::{CAPABILITIES, Opcode, frame};
use tokio_tungstenite::tungstenite::Message;

use super::transport::{Socket, connect};
use super::{ready, start, terminate};

struct Client {
    socket: Socket,
    frames: Vec<Vec<u8>>,
    events: Vec<Value>,
    sequence: usize,
}

impl Client {
    async fn connect(address: &str) -> Self {
        let mut methods = CAPABILITIES.to_vec();
        methods.extend([
            "workspace.open.request",
            "workspace.archive.request",
            "agent.items.close.request",
            "subscription.release.request",
            "connection.ping",
        ]);
        Self {
            socket: connect(address, &methods).await,
            frames: Vec::new(),
            events: Vec::new(),
            sequence: 0,
        }
    }

    async fn request(&mut self, method: &str, params: Value) -> Value {
        self.sequence += 1;
        let id = self.sequence.to_string();
        self.send(json!({"type":"request","request_id":id,"method":method,"params":params}))
            .await;
        loop {
            let value = self.next().await;
            if value["request_id"] == id {
                return value;
            }
            self.events.push(value);
        }
    }

    async fn send(&mut self, value: Value) {
        self.socket
            .send(Message::Text(value.to_string().into()))
            .await
            .unwrap();
    }

    async fn next(&mut self) -> Value {
        loop {
            match tokio::time::timeout(Duration::from_secs(10), self.socket.next())
                .await
                .unwrap()
                .unwrap()
                .unwrap()
            {
                Message::Text(text) => return serde_json::from_str(&text).unwrap(),
                Message::Binary(bytes) => self.frames.push(bytes.to_vec()),
                message => panic!("unexpected message: {message:?}"),
            }
        }
    }

    async fn capture(&mut self, terminal: &Value, needle: &str) -> Value {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        loop {
            let response = self
                .request("terminal.capture.request", json!({"terminalId":terminal}))
                .await;
            if response["result"]["lines"].to_string().contains(needle) {
                return response["result"].clone();
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "capture timeout: {response}"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    async fn input(&mut self, terminal: &Value, data: &str) {
        self.send(json!({"type":"event","method":"terminal.input","params":{"terminalId":terminal,"message":{"type":"input","data":data}}})).await;
    }
}

async fn create(client: &mut Client, cwd: &std::path::Path) -> Value {
    let response = client.request("terminal.create.request", json!({"cwd":cwd,"command":"/bin/sh","args":["-c","printf 'READY\\n'; while read line; do stty size; printf 'READ:%s\\n' \"$line\"; done"]})).await;
    assert!(response["result"]["error"].is_null(), "{response}");
    response["result"]["terminal"]["id"].clone()
}

#[tokio::test]
async fn terminal_methods_stream_frames_and_connection_owned_release_work_end_to_end() {
    let temp = tempfile::tempdir().unwrap();
    let cwd = temp.path().join("workspace");
    std::fs::create_dir(&cwd).unwrap();
    let log = temp.path().join("log");
    let mut server = start(&temp.path().join("state"), &log);
    let address = ready(&mut server, &log).await;
    let mut client = Client::connect(&address).await;
    let opened = client
        .request("workspace.open.request", json!({"cwd":cwd}))
        .await;
    let workspace = opened["result"]["workspace"]["id"].clone();
    let terminal = create(&mut client, &cwd).await;
    client.capture(&terminal, "READY").await;
    let listed = client.request("terminal.list.request", json!({})).await;
    assert_eq!(listed["result"]["terminals"][0]["id"], terminal);
    let listing = client
        .request("terminal.list.subscribe.request", json!({"cwd":cwd}))
        .await;
    let subscribed = client
        .request("terminal.subscribe.request", json!({"terminalId":terminal}))
        .await;
    let slot = u8::try_from(subscribed["result"]["slot"].as_u64().unwrap()).unwrap();
    client.input(&terminal, "json-input\r").await;
    client.capture(&terminal, "READ:json-input").await;
    assert!(client.frames.iter().any(|bytes| bytes[..2] == [4, slot]));
    client
        .socket
        .send(Message::Binary(
            frame(Opcode::Input, slot, b"binary-input\r").into(),
        ))
        .await
        .unwrap();
    client.capture(&terminal, "READ:binary-input").await;
    let renamed = client
        .request(
            "terminal.rename.request",
            json!({"terminalId":terminal,"title":"new terminal"}),
        )
        .await;
    assert_eq!(renamed["result"]["success"], true);
    tokio::time::sleep(Duration::from_millis(150)).await;
    client
        .request("connection.ping", json!({"nonce":"flush"}))
        .await;
    assert!(
        client
            .events
            .iter()
            .any(|event| event["method"] == "terminal.list.changed"
                && event["params"]["terminals"][0]["title"] == "new terminal")
    );
    assert!(client.frames.iter().any(|bytes| bytes[..2] == [1, slot]));
    let size_snapshot = assert_resize_ownership(&mut client, &address, &terminal, slot).await;
    client
        .request(
            "subscription.release.request",
            json!({"subscriptionId":size_snapshot["result"]["subscriptionId"]}),
        )
        .await;
    client
        .request(
            "subscription.release.request",
            json!({"subscriptionId":listing["result"]["subscriptionId"]}),
        )
        .await;
    client.frames.clear();
    client.input(&terminal, "released\r").await;
    client.capture(&terminal, "READ:released").await;
    assert!(client.frames.is_empty());
    let restored = client.request("terminal.subscribe.request", json!({"terminalId":terminal,"restore":{"mode":"visible-snapshot","scrollbackLines":5}})).await;
    assert!(restored["result"]["error"].is_null());
    client
        .request("connection.ping", json!({"nonce":"restore"}))
        .await;
    assert!(
        client
            .frames
            .iter()
            .any(|bytes| bytes[0] == 5 && bytes.len() > 2)
    );
    assert_killed_terminal(&mut client, &terminal).await;
    assert!(!workspace.is_null(), "{opened}");
    client.socket.close(None).await.unwrap();
    terminate(&mut server).await;
}

async fn assert_killed_terminal(client: &mut Client, terminal: &Value) {
    let killed = client
        .request("terminal.kill.request", json!({"terminalId":terminal}))
        .await;
    assert_eq!(killed["result"]["success"], true);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while !client
        .events
        .iter()
        .any(|event| event["method"] == "terminal.stream.exit")
    {
        assert!(tokio::time::Instant::now() < deadline);
        let event = client.next().await;
        client.events.push(event);
    }
    assert_eq!(
        client
            .request("terminal.capture.request", json!({"terminalId":terminal}))
            .await["result"]["totalLines"],
        0
    );
}

async fn assert_resize_ownership(
    client: &mut Client,
    address: &str,
    terminal: &Value,
    slot: u8,
) -> Value {
    let mut other = Client::connect(address).await;
    other
        .socket
        .send(Message::Binary(
            frame(Opcode::Input, slot, b"forbidden\r").into(),
        ))
        .await
        .unwrap();
    assert_eq!(other.next().await["code"], "subscription_not_found");
    let resize = br#"{"rows":31,"cols":91,"intent":"claim"}"#;
    client
        .socket
        .send(Message::Binary(frame(Opcode::Resize, slot, resize).into()))
        .await
        .unwrap();
    client.input(terminal, "sized\r").await;
    client.capture(terminal, "31 91").await;
    other.send(json!({"type":"event","method":"terminal.input","params":{"terminalId":terminal,"message":{"type":"resize","rows":10,"cols":20,"intent":"update"}}})).await;
    other
        .request("connection.ping", json!({"nonce":"resize-processed"}))
        .await;
    client.input(terminal, "owner-retained\r").await;
    client.capture(terminal, "READ:owner-retained").await;
    let size_snapshot = client
        .request("terminal.subscribe.request", json!({"terminalId":terminal}))
        .await;
    let replacement_slot = u8::try_from(size_snapshot["result"]["slot"].as_u64().unwrap()).unwrap();
    client
        .request("connection.ping", json!({"nonce":"snapshot"}))
        .await;
    let state: Value = serde_json::from_slice(
        &client
            .frames
            .iter()
            .find(|bytes| bytes[..2] == [4, replacement_slot])
            .unwrap()[2..],
    )
    .unwrap();
    assert_eq!(state["rows"], 31);
    other.socket.close(None).await.unwrap();
    size_snapshot
}

#[tokio::test]
async fn terminal_reconnect_archive_batch_close_and_shutdown_cleanup_are_observable() {
    let temp = tempfile::tempdir().unwrap();
    let cwd = temp.path().join("workspace");
    std::fs::create_dir(&cwd).unwrap();
    let log = temp.path().join("log");
    let mut server = start(&temp.path().join("state"), &log);
    let address = ready(&mut server, &log).await;
    let mut client = Client::connect(&address).await;
    let opened = client
        .request("workspace.open.request", json!({"cwd":cwd}))
        .await;
    let workspace = opened["result"]["workspace"]["id"].clone();
    let terminal = create(&mut client, &cwd).await;
    client.input(&terminal, "survives-disconnect\r").await;
    client.capture(&terminal, "READ:survives-disconnect").await;
    client.socket.close(None).await.unwrap();
    let mut client = Client::connect(&address).await;
    client.capture(&terminal, "READ:survives-disconnect").await;
    let closed = client
        .request(
            "agent.items.close.request",
            json!({"agentIds":[],"terminalIds":[terminal]}),
        )
        .await;
    assert_eq!(
        closed["result"]["terminals"][0]["success"], true,
        "{closed}"
    );
    let terminal = create(&mut client, &cwd).await;
    let archived = client
        .request(
            "workspace.archive.request",
            json!({"workspaceId":workspace}),
        )
        .await;
    assert!(archived["result"]["error"].is_null(), "{archived}");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let capture = client
            .request("terminal.capture.request", json!({"terminalId":terminal}))
            .await;
        if capture["result"]["totalLines"] == 0 {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "terminal remained after workspace archive"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let opened = client
        .request("workspace.open.request", json!({"cwd":cwd}))
        .await;
    assert!(opened["result"]["error"].is_null());
    let pid_file = temp.path().join("terminal.pid");
    let created = client.request("terminal.create.request", json!({"cwd":cwd,"command":"/bin/sh","args":["-c",format!("echo $$ > '{}'; sleep 60",pid_file.display())]})).await;
    assert!(created["result"]["error"].is_null(), "{created}");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while !pid_file.exists() {
        assert!(tokio::time::Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let pid = std::fs::read_to_string(pid_file)
        .unwrap()
        .trim()
        .parse::<i32>()
        .unwrap();
    client.socket.close(None).await.unwrap();
    terminate(&mut server).await;
    assert!(
        !std::process::Command::new("kill")
            .args(["-0", &pid.to_string()])
            .output()
            .unwrap()
            .status
            .success()
    );
}

#[tokio::test]
async fn terminal_unsubscribe_validation_and_natural_exit_preserve_protocol_direction() {
    let temp = tempfile::tempdir().unwrap();
    let cwd = temp.path().join("workspace");
    std::fs::create_dir(&cwd).unwrap();
    let log = temp.path().join("log");
    let mut server = start(&temp.path().join("state"), &log);
    let address = ready(&mut server, &log).await;
    let mut client = Client::connect(&address).await;
    client
        .request("workspace.open.request", json!({"cwd":cwd}))
        .await;
    assert_eq!(
        client.request("terminal.input", json!({})).await["code"],
        "invalid_message"
    );
    assert_eq!(
        client
            .request("terminal.list.subscribe.request", json!({}))
            .await["code"],
        "invalid_message"
    );
    assert!(
        client
            .request(
                "terminal.subscribe.request",
                json!({"terminalId":"missing"})
            )
            .await["result"]["error"]
            .is_string()
    );
    assert!(
        client
            .request(
                "terminal.create.request",
                json!({"cwd":cwd,"size":{"rows":0,"cols":80}})
            )
            .await["result"]["error"]
            .is_string()
    );
    let terminal = create(&mut client, &cwd).await;
    client
        .request("terminal.list.subscribe.request", json!({"cwd":cwd}))
        .await;
    let subscribed = client
        .request(
            "terminal.subscribe.request",
            json!({"terminalId":terminal,"restore":{"mode":"live"}}),
        )
        .await;
    client
        .request(
            "terminal.unsubscribe.request",
            json!({"terminalId":terminal}),
        )
        .await;
    client
        .request("terminal.list.unsubscribe.request", json!({"cwd":cwd}))
        .await;
    client.frames.clear();
    client.events.clear();
    client.input(&terminal, "after-release\r").await;
    client.capture(&terminal, "READ:after-release").await;
    assert!(client.frames.is_empty());
    assert!(client.events.is_empty());
    let slot = u8::try_from(subscribed["result"]["slot"].as_u64().unwrap()).unwrap();
    client
        .socket
        .send(Message::Binary(
            frame(Opcode::Input, slot, b"stale\r").into(),
        ))
        .await
        .unwrap();
    assert_eq!(client.next().await["code"], "subscription_not_found");
    client
        .socket
        .send(Message::Binary(
            frame(Opcode::Output, slot, b"wrong-direction").into(),
        ))
        .await
        .unwrap();
    assert_eq!(client.next().await["code"], "invalid_message");
    client
        .request("terminal.kill.request", json!({"terminalId":terminal}))
        .await;
    assert_natural_exit(&mut client, &cwd).await;
    let mut unnegotiated = connect(&address, &["connection.ping"]).await;
    unnegotiated
        .send(Message::Binary(
            frame(Opcode::Input, 0, b"unnegotiated").into(),
        ))
        .await
        .unwrap();
    assert_eq!(
        super::transport::receive(&mut unnegotiated).await["code"],
        "unsupported_capability"
    );
    unnegotiated.close(None).await.unwrap();
    client.socket.close(None).await.unwrap();
    terminate(&mut server).await;
}

async fn assert_natural_exit(client: &mut Client, cwd: &std::path::Path) {
    let created = client.request("terminal.create.request", json!({"cwd":cwd,"command":"/bin/sh","args":["-c","read line; printf 'FINAL_OUTPUT\\n'"]})).await;
    let terminal = created["result"]["terminal"]["id"].clone();
    assert!(terminal.is_string(), "{created}");
    client
        .request(
            "terminal.subscribe.request",
            json!({"terminalId":terminal,"restore":{"mode":"full-snapshot"}}),
        )
        .await;
    client.input(&terminal, "exit\r").await;
    loop {
        let message = client.next().await;
        if message["method"] == "terminal.stream.exit" {
            break;
        }
    }
    assert!(
        client
            .frames
            .iter()
            .any(|bytes| String::from_utf8_lossy(&bytes[2..]).contains("FINAL_OUTPUT"))
    );
}
