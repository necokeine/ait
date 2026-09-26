//! Real-server counterparts of Paseo owned-subscriptions and trailing-output tests.

use super::*;

struct Fixture {
    server: super::super::Process,
    client: Client,
    terminal: Value,
    cwd: std::path::PathBuf,
    address: String,
    _root: tempfile::TempDir,
}

impl Fixture {
    async fn start() -> Self {
        let root = tempfile::tempdir().unwrap();
        let cwd = root.path().join("workspace");
        std::fs::create_dir(&cwd).unwrap();
        let log = root.path().join("server.log");
        let mut server = start(&root.path().join("state"), &log);
        let address = ready(&mut server, &log).await;
        let mut client = Client::connect(&address).await;
        let opened = client
            .request("workspace.open.request", json!({"cwd":cwd}))
            .await;
        assert!(opened["result"]["workspace"]["id"].is_string());
        let terminal = create(&mut client, &cwd).await;
        client.capture(&terminal, "READY").await;
        Self {
            server,
            client,
            terminal,
            cwd,
            address,
            _root: root,
        }
    }

    async fn subscribe(&mut self) -> Value {
        let reply = self
            .client
            .request(
                "terminal.subscribe.request",
                json!({"terminalId":self.terminal}),
            )
            .await;
        assert!(reply["result"]["error"].is_null(), "{reply}");
        self.client
            .request("connection.ping", json!({"nonce":"bootstrap-fence"}))
            .await;
        reply["result"].clone()
    }

    async fn stop(mut self) {
        self.client.socket.close(None).await.unwrap();
        terminate(&mut self.server).await;
    }
}

fn slot(reply: &Value) -> u8 {
    u8::try_from(reply["slot"].as_u64().unwrap()).unwrap()
}

fn output(client: &Client, slot: u8) -> Vec<u8> {
    client
        .frames
        .iter()
        .filter(|bytes| bytes.starts_with(&[Opcode::Output as u8, slot]))
        .flat_map(|bytes| bytes[2..].iter().copied())
        .collect()
}

async fn await_output(client: &mut Client, slots: &[u8], needle: &str) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        client
            .request("connection.ping", json!({"nonce":"output-fence"}))
            .await;
        if slots
            .iter()
            .all(|slot| String::from_utf8_lossy(&output(client, *slot)).contains(needle))
        {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "missing output {needle}"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[tokio::test]
async fn duplicate_terminal_observers_deliver_both_slots_and_release_the_newest_independently() {
    let mut fixture = Fixture::start().await;
    let mut idle = Client::connect(&fixture.address).await;
    let first = fixture.subscribe().await;
    let second = fixture.subscribe().await;
    assert_ne!(first["subscriptionId"], second["subscriptionId"]);
    assert_ne!(slot(&first), slot(&second));
    fixture.client.frames.clear();
    fixture
        .client
        .input(&fixture.terminal, "shared-output\r")
        .await;
    await_output(
        &mut fixture.client,
        &[slot(&first), slot(&second)],
        "READ:shared-output",
    )
    .await;
    fixture
        .client
        .request(
            "subscription.release.request",
            json!({"subscriptionId":second["subscriptionId"]}),
        )
        .await;
    fixture.client.frames.clear();
    fixture
        .client
        .input(&fixture.terminal, "surviving-original\r")
        .await;
    await_output(
        &mut fixture.client,
        &[slot(&first)],
        "READ:surviving-original",
    )
    .await;
    assert!(
        fixture
            .client
            .frames
            .iter()
            .all(|bytes| bytes[1] == slot(&first))
    );
    idle.request("connection.ping", json!({"nonce":"idle-fence"}))
        .await;
    assert!(idle.frames.is_empty());
    assert!(idle.events.is_empty());
    idle.socket.close(None).await.unwrap();
    fixture.stop().await;
}

#[tokio::test]
async fn duplicate_terminal_directory_observers_keep_the_original_after_releasing_the_newest() {
    let mut fixture = Fixture::start().await;
    let first = fixture
        .client
        .request(
            "terminal.list.subscribe.request",
            json!({"cwd":fixture.cwd}),
        )
        .await;
    let second = fixture
        .client
        .request(
            "terminal.list.subscribe.request",
            json!({"cwd":fixture.cwd}),
        )
        .await;
    fixture
        .client
        .request(
            "subscription.release.request",
            json!({"subscriptionId":second["result"]["subscriptionId"]}),
        )
        .await;
    fixture.client.events.clear();
    fixture
        .client
        .request(
            "terminal.rename.request",
            json!({"terminalId":fixture.terminal,"title":"independent-observer"}),
        )
        .await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while fixture.client.events.is_empty() {
        fixture
            .client
            .request("connection.ping", json!({"nonce":"directory-fence"}))
            .await;
        assert!(tokio::time::Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(fixture.client.events.len(), 1);
    let changed = &fixture.client.events[0];
    assert_eq!(changed["method"], "terminal.list.changed");
    assert_eq!(
        changed["params"]["subscriptionId"],
        first["result"]["subscriptionId"]
    );
    assert_eq!(
        changed["params"]["terminals"][0]["title"],
        "independent-observer"
    );
    fixture.stop().await;
}

#[tokio::test]
async fn duplicate_terminal_observers_are_bounded_and_release_reopens_one_slot() {
    let mut fixture = Fixture::start().await;
    let mut leases = Vec::new();
    for _ in 0..16 {
        leases.push(fixture.subscribe().await);
    }
    let refused = fixture
        .client
        .request(
            "terminal.subscribe.request",
            json!({"terminalId":fixture.terminal}),
        )
        .await;
    assert_eq!(refused["code"], "resource_exhausted");
    fixture
        .client
        .request(
            "subscription.release.request",
            json!({"subscriptionId":leases[7]["subscriptionId"]}),
        )
        .await;
    fixture
        .client
        .socket
        .send(Message::Binary(
            frame(Opcode::Input, slot(&leases[7]), b"released-slot\r").into(),
        ))
        .await
        .unwrap();
    assert_eq!(
        fixture.client.next().await["code"],
        "subscription_not_found"
    );
    let replacement = fixture.subscribe().await;
    assert!(
        leases
            .iter()
            .all(|lease| lease["subscriptionId"] != replacement["subscriptionId"])
    );
    fixture.stop().await;
}

#[tokio::test]
async fn final_pty_output_is_delivered_before_the_natural_exit_event() {
    let mut fixture = Fixture::start().await;
    let created = fixture.client.request("terminal.create.request", json!({"cwd":fixture.cwd,"command":"/bin/sh","args":["-c","printf 'WAITING\\n'; read line; printf 'LAST:%s\\n' \"$line\""]})).await;
    let terminal = created["result"]["terminal"]["id"].clone();
    assert!(terminal.is_string());
    fixture.client.capture(&terminal, "WAITING").await;
    let lease = fixture
        .client
        .request("terminal.subscribe.request", json!({"terminalId":terminal}))
        .await["result"]
        .clone();
    fixture
        .client
        .request("connection.ping", json!({"nonce":"subscribed"}))
        .await;
    fixture.client.frames.clear();
    fixture.client.input(&terminal, "final-marker\r").await;
    loop {
        let message = fixture.client.next().await;
        if message["method"] == "terminal.stream.exit" {
            assert_eq!(message["params"]["subscriptionId"], lease["subscriptionId"]);
            break;
        }
        fixture.client.events.push(message);
    }
    assert!(
        String::from_utf8_lossy(&output(&fixture.client, slot(&lease)))
            .contains("LAST:final-marker")
    );
    fixture
        .client
        .request("connection.ping", json!({"nonce":"exit-fence"}))
        .await;
    assert!(
        !fixture
            .client
            .events
            .iter()
            .any(|event| event["method"] == "terminal.stream.exit")
    );
    fixture.stop().await;
}
