use super::*;

#[tokio::test]
async fn identical_output_observers_have_independent_ids_slots_and_lifetimes() {
    let mut fixture = Fixture::new();
    let first = fixture.subscribe().await;
    assert_eq!(
        fixture.binary(),
        wire::frame(Opcode::Snapshot, slot(&first), b"bootstrap")
    );
    let second = fixture.subscribe().await;
    fixture.binary();
    assert_ne!(subscription(&first), subscription(&second));
    assert_ne!(slot(&first), slot(&second));
    assert_eq!(fixture.connection.len(), 2);
    fixture.output(b"both", false);
    fixture.poll().await;
    let mut delivered = [fixture.binary(), fixture.binary()];
    delivered.sort_unstable();
    let mut expected = [
        wire::frame(Opcode::Output, slot(&first), b"both"),
        wire::frame(Opcode::Output, slot(&second), b"both"),
    ];
    expected.sort_unstable();
    assert_eq!(delivered, expected);
    fixture.connection.release(subscription(&first));
    fixture.output(b"remaining", false);
    fixture.poll().await;
    assert_eq!(
        fixture.binary(),
        wire::frame(Opcode::Output, slot(&second), b"remaining")
    );
    fixture.assert_quiet();
}

#[tokio::test]
async fn identical_directory_observers_release_independently() {
    let mut fixture = Fixture::new();
    let first = fixture
        .request(
            "terminal.list.subscribe.request",
            json!({"cwd":"/repo"}),
            16,
        )
        .await;
    let second = fixture
        .request(
            "terminal.list.subscribe.request",
            json!({"cwd":"/repo"}),
            15,
        )
        .await;
    assert_ne!(
        first["result"]["subscriptionId"],
        second["result"]["subscriptionId"]
    );
    assert_eq!(fixture.connection.len(), 2);
    fixture.connection.release(subscription(&first["result"]));
    fixture.calls.lock().unwrap().title = Some("changed".to_owned());
    fixture.poll().await;
    let event = fixture.json();
    assert_eq!(event["method"], "terminal.list.changed");
    assert_eq!(
        event["params"]["subscriptionId"],
        second["result"]["subscriptionId"]
    );
    assert_eq!(event["params"]["terminals"][0]["title"], "changed");
    fixture.assert_quiet();
}

#[tokio::test]
async fn repeated_output_subscription_cannot_bypass_the_connection_budget() {
    let mut fixture = Fixture::new();
    let original = fixture.subscribe().await;
    fixture.binary();
    let reply = fixture
        .request(
            "terminal.subscribe.request",
            json!({"terminalId":fixture.terminal}),
            0,
        )
        .await;
    assert_eq!(reply["code"], "resource_exhausted");
    assert_eq!(fixture.connection.len(), 1);
    fixture.output(b"still owned", false);
    fixture.poll().await;
    assert_eq!(
        fixture.binary(),
        wire::frame(Opcode::Output, slot(&original), b"still owned")
    );
}

#[tokio::test]
async fn repeated_directory_subscription_cannot_bypass_the_connection_budget() {
    let mut fixture = Fixture::new();
    fixture
        .request("terminal.list.subscribe.request", json!({"cwd":"/repo"}), 1)
        .await;
    let reply = fixture
        .request("terminal.list.subscribe.request", json!({"cwd":"/repo"}), 0)
        .await;
    assert_eq!(reply["code"], "resource_exhausted");
    assert_eq!(fixture.connection.len(), 1);
    fixture.assert_quiet();
}

#[tokio::test]
async fn pure_list_and_capture_requests_create_no_observers() {
    let mut fixture = Fixture::new();
    fixture
        .request("terminal.list.request", json!({"cwd":"/repo"}), 0)
        .await;
    fixture
        .request(
            "terminal.capture.request",
            json!({"terminalId":fixture.terminal}),
            0,
        )
        .await;
    assert!(fixture.connection.is_empty());
    fixture.calls.lock().unwrap().title = Some("changed".to_owned());
    fixture.output(b"unobserved", false);
    fixture.poll().await;
    fixture.assert_quiet();
}

#[tokio::test]
async fn releasing_a_foreign_id_does_not_remove_owned_streams() {
    let mut fixture = Fixture::new();
    let lease = fixture.subscribe().await;
    fixture.binary();
    fixture.connection.release("foreign-subscription");
    fixture.output(b"owned", false);
    fixture.poll().await;
    assert_eq!(
        fixture.binary(),
        wire::frame(Opcode::Output, slot(&lease), b"owned")
    );
    assert_eq!(fixture.connection.len(), 1);
}

#[tokio::test]
async fn released_slots_reject_binary_input_without_affecting_sibling_observers() {
    let mut fixture = Fixture::new();
    let first = fixture.subscribe().await;
    fixture.binary();
    let second = fixture.subscribe().await;
    fixture.binary();
    fixture.connection.release(subscription(&first));
    assert_eq!(
        fixture
            .connection
            .binary(
                &wire::frame(Opcode::Input, slot(&first), b"stale"),
                &fixture.state
            )
            .await,
        Err(ErrorCode::SubscriptionNotFound)
    );
    fixture
        .connection
        .binary(
            &wire::frame(Opcode::Input, slot(&second), b"valid"),
            &fixture.state,
        )
        .await
        .unwrap();
    assert_eq!(
        fixture.calls.lock().unwrap().inputs,
        ["Input { data: \"valid\" }"]
    );
}

#[tokio::test]
async fn another_physical_connection_cannot_use_an_owners_binary_slot() {
    let mut fixture = Fixture::new();
    let lease = fixture.subscribe().await;
    fixture.binary();
    let mut stranger = TerminalConnection::default();
    assert_eq!(
        stranger
            .binary(
                &wire::frame(Opcode::Input, slot(&lease), b"foreign"),
                &fixture.state
            )
            .await,
        Err(ErrorCode::SubscriptionNotFound)
    );
    assert!(fixture.calls.lock().unwrap().inputs.is_empty());
}

#[tokio::test]
async fn legacy_unsubscribe_releases_all_output_observers_for_the_target_only() {
    let mut fixture = Fixture::new();
    fixture.subscribe().await;
    fixture.binary();
    let other_terminal = fixture
        .state
        .terminals
        .as_ref()
        .unwrap()
        .lock()
        .unwrap()
        .create(&request())
        .unwrap()
        .id;
    let other = fixture
        .request(
            "terminal.subscribe.request",
            json!({"terminalId":other_terminal}),
            14,
        )
        .await["result"]
        .clone();
    fixture.binary();
    fixture.subscribe().await;
    fixture.binary();
    fixture
        .request(
            "terminal.list.subscribe.request",
            json!({"cwd":"/repo"}),
            14,
        )
        .await;
    fixture
        .request(
            "terminal.unsubscribe.request",
            json!({"terminalId":fixture.terminal}),
            13,
        )
        .await;
    assert_eq!(fixture.connection.len(), 2);
    fixture.output(b"released", false);
    fixture.poll().await;
    assert_eq!(
        fixture.binary(),
        wire::frame(Opcode::Output, slot(&other), b"released")
    );
    fixture.assert_quiet();
}

#[tokio::test]
async fn failed_subscribe_does_not_allocate_a_slot_or_consume_an_observer() {
    let mut fixture = Fixture::new();
    fixture.calls.lock().unwrap().observe_error = Some(crate::Error::Io);
    let reply = fixture
        .request(
            "terminal.subscribe.request",
            json!({"terminalId":fixture.terminal}),
            16,
        )
        .await;
    assert!(reply["result"]["error"].is_string());
    assert!(reply["result"]["subscriptionId"].is_null());
    assert!(fixture.connection.is_empty());
    fixture.calls.lock().unwrap().observe_error = None;
    let success = fixture.subscribe().await;
    assert_eq!(slot(&success), 0);
    fixture.binary();
    fixture.assert_quiet();
}

#[tokio::test]
async fn failed_attach_resize_creates_no_observer_and_keeps_input_errors_visible() {
    let mut fixture = Fixture::new();
    fixture.calls.lock().unwrap().send_failure = true;
    let reply = fixture.request("terminal.subscribe.request", json!({"terminalId":fixture.terminal,"restore":{"mode":"live","size":{"rows":30,"cols":100}}}), 16).await;
    assert_eq!(reply["result"]["error"], ErrorCode::TerminalIo.message());
    assert!(fixture.connection.is_empty());
    assert!(fixture.calls.lock().unwrap().inputs.is_empty());
}
