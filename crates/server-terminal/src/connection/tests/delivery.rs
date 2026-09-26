use super::*;

#[tokio::test]
async fn every_observer_receives_final_bytes_before_its_single_exit() {
    let mut fixture = Fixture::new();
    let first = fixture.subscribe().await;
    fixture.binary();
    let second = fixture.subscribe().await;
    fixture.binary();
    fixture.output(b"final for both", true);
    fixture.poll().await;
    let mut exited = Vec::new();
    for _ in 0..2 {
        let output = fixture.binary();
        let exit = fixture.json();
        let lease = if exit["params"]["subscriptionId"] == first["subscriptionId"] {
            &first
        } else {
            &second
        };
        assert_eq!(
            output,
            wire::frame(Opcode::Output, slot(lease), b"final for both")
        );
        assert_eq!(exit["method"], "terminal.stream.exit");
        assert_eq!(exit["params"]["terminalId"], fixture.terminal);
        exited.push(
            exit["params"]["subscriptionId"]
                .as_str()
                .unwrap()
                .to_owned(),
        );
    }
    exited.sort_unstable();
    let mut expected = [subscription(&first), subscription(&second)];
    expected.sort_unstable();
    assert_eq!(exited, expected);
    assert!(fixture.connection.is_empty());
    fixture.poll().await;
    fixture.assert_quiet();
}

#[tokio::test]
async fn response_precedes_bootstrap_and_output_precedes_natural_exit() {
    let mut fixture = Fixture::new();
    let lease = fixture.subscribe().await;
    assert_eq!(
        fixture.binary(),
        wire::frame(Opcode::Snapshot, slot(&lease), b"bootstrap")
    );
    fixture.output(b"final output", true);
    fixture.poll().await;
    assert_eq!(
        fixture.binary(),
        wire::frame(Opcode::Output, slot(&lease), b"final output")
    );
    let exit = fixture.json();
    assert_eq!(exit["method"], "terminal.stream.exit");
    assert_eq!(exit["params"]["subscriptionId"], lease["subscriptionId"]);
    assert_eq!(exit["params"]["terminalId"], fixture.terminal);
    assert!(fixture.connection.is_empty());
    fixture.poll().await;
    fixture.assert_quiet();
}

#[tokio::test]
async fn already_drained_terminal_sends_snapshot_then_exit_without_retaining_observer() {
    let mut fixture = Fixture::new();
    fixture
        .calls
        .lock()
        .unwrap()
        .observation
        .as_mut()
        .unwrap()
        .exited = true;
    let lease = fixture.subscribe().await;
    fixture.binary();
    assert_eq!(
        fixture.json()["params"]["subscriptionId"],
        lease["subscriptionId"]
    );
    assert!(fixture.connection.is_empty());
    fixture.assert_quiet();
}

#[tokio::test]
async fn release_discards_only_that_observers_pending_exit_and_output() {
    let mut fixture = Fixture::new();
    let first = fixture.subscribe().await;
    fixture.binary();
    let second = fixture.subscribe().await;
    fixture.binary();
    fixture.output(b"last", true);
    fixture.connection.release(subscription(&first));
    fixture.poll().await;
    assert_eq!(
        fixture.binary(),
        wire::frame(Opcode::Output, slot(&second), b"last")
    );
    assert_eq!(
        fixture.json()["params"]["subscriptionId"],
        second["subscriptionId"]
    );
    fixture.assert_quiet();
}

#[tokio::test]
async fn resize_notification_is_delivered_before_output_at_the_new_size() {
    let mut fixture = Fixture::new();
    let lease = fixture.subscribe().await;
    fixture.binary();
    fixture.output(b"resized", false);
    fixture
        .calls
        .lock()
        .unwrap()
        .observation
        .as_mut()
        .unwrap()
        .size = wire::Size {
        rows: 30,
        cols: 100,
    };
    fixture.poll().await;
    let resized = fixture.binary();
    assert_eq!(&resized[..2], &[Opcode::Resize as u8, slot(&lease)]);
    assert_eq!(
        serde_json::from_slice::<Value>(&resized[2..]).unwrap(),
        json!({"rows":30,"cols":100})
    );
    assert_eq!(
        fixture.binary(),
        wire::frame(Opcode::Output, slot(&lease), b"resized")
    );
    fixture.poll().await;
    fixture.assert_quiet();
}

#[tokio::test]
async fn snapshot_failure_emits_one_owned_exit_and_removes_the_failed_stream() {
    let mut fixture = Fixture::new();
    let lease = fixture.subscribe().await;
    fixture.binary();
    fixture.calls.lock().unwrap().observe_error = Some(crate::Error::Io);
    fixture.poll().await;
    let exit = fixture.json();
    assert_eq!(exit["method"], "terminal.stream.exit");
    assert_eq!(exit["params"]["subscriptionId"], lease["subscriptionId"]);
    assert_eq!(exit["params"]["error"], crate::Error::Io.to_string());
    assert!(fixture.connection.is_empty());
    fixture.poll().await;
    fixture.assert_quiet();
}

#[tokio::test]
async fn missing_terminal_exits_without_an_internal_error_or_sibling_list_loss() {
    let mut fixture = Fixture::new();
    fixture.subscribe().await;
    fixture.binary();
    fixture
        .request(
            "terminal.list.subscribe.request",
            json!({"cwd":"/repo"}),
            15,
        )
        .await;
    fixture
        .state
        .terminals
        .as_ref()
        .unwrap()
        .lock()
        .unwrap()
        .shutdown()
        .unwrap();
    fixture.poll().await;
    let exit = fixture.json();
    assert_eq!(exit["method"], "terminal.stream.exit");
    assert!(exit["params"].get("error").is_none());
    let list = fixture.json();
    assert_eq!(list["method"], "terminal.list.changed");
    assert_eq!(list["params"]["terminals"], json!([]));
    assert_eq!(fixture.connection.len(), 1);
}

#[tokio::test]
async fn exhausted_io_budget_keeps_the_cursor_and_retries_without_losing_output() {
    let mut fixture = Fixture::new();
    let lease = fixture.subscribe().await;
    fixture.binary();
    let permit = fixture
        .state
        .terminal_jobs
        .clone()
        .acquire_many_owned(4)
        .await
        .unwrap();
    fixture.output(b"retained", false);
    fixture.poll().await;
    fixture.assert_quiet();
    assert_eq!(fixture.connection.len(), 1);
    drop(permit);
    fixture.poll().await;
    assert_eq!(
        fixture.binary(),
        wire::frame(Opcode::Output, slot(&lease), b"retained")
    );
    fixture.poll().await;
    fixture.assert_quiet();
}

#[tokio::test]
async fn unchanged_directory_snapshot_does_not_emit_redundant_events() {
    let mut fixture = Fixture::new();
    fixture
        .request(
            "terminal.list.subscribe.request",
            json!({"cwd":"/repo"}),
            16,
        )
        .await;
    for _ in 0..3 {
        fixture.poll().await;
    }
    fixture.assert_quiet();
    fixture.calls.lock().unwrap().title = Some("changed".to_owned());
    fixture.poll().await;
    assert_eq!(fixture.json()["params"]["terminals"][0]["title"], "changed");
    fixture.poll().await;
    fixture.assert_quiet();
}
