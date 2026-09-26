//! Bounded variants of Paseo broker admission, fanout and cancellation scenarios.
use super::*;

#[test]
fn host_capacity_is_reusable_after_a_registration_is_released() {
    let broker = Broker::default();
    let mut registrations: Vec<_> = (0..128).map(|_| host(&broker, &["list_tabs"])).collect();
    let (outbound, _receive) = Outbound::new();
    assert_eq!(
        broker
            .register(
                Register {
                    host_kind: "desktop".into(),
                    supported_commands: vec!["list_tabs".into()]
                },
                outbound
            )
            .unwrap_err(),
        ErrorCode::ResourceExhausted
    );
    registrations.pop();
    let (_lease, _receive) = host(&broker, &["list_tabs"]);
    assert_eq!(broker.0.lock().unwrap().hosts.len(), 128);
}

#[tokio::test]
async fn pending_capacity_rejects_without_delivery_and_is_reusable_after_cancel() {
    let broker = Broker::default();
    let (lease, mut receive) = host(&broker, &["list_tabs"]);
    let mut tasks = Vec::new();
    for _ in 0..128 {
        let runner = broker.clone();
        tasks.push(tokio::spawn(async move {
            runner
                .execute(
                    json!({"command":"list_tabs"}),
                    json!({}),
                    Duration::from_secs(30),
                )
                .await
        }));
        let _ = request(&mut receive).await;
    }
    assert_eq!(broker.0.lock().unwrap().pending.len(), 128);
    let rejected = spawn(&broker, json!({"command":"list_tabs"}))
        .await
        .unwrap();
    assert_eq!(rejected["error"]["code"], "browser_unknown_error");
    assert_eq!(rejected["error"]["retryable"], true);
    assert!(receive.try_recv().is_err());
    let canceled = tasks.pop().unwrap();
    canceled.abort();
    assert!(canceled.await.unwrap_err().is_cancelled());
    let replacement = spawn(&broker, json!({"command":"list_tabs"}));
    let call = request(&mut receive).await;
    answer(
        &broker,
        &lease,
        &call,
        &json!({"command":"list_tabs","tabs":[]}),
    );
    assert_eq!(replacement.await.unwrap()["ok"], true);
    drop(lease);
    for task in tasks {
        assert_eq!(task.await.unwrap()["error"]["code"], "browser_no_host");
    }
    assert!(broker.0.lock().unwrap().pending.is_empty());
}

#[tokio::test]
async fn list_fanout_refuses_aggregate_over_tab_limit_without_learning_affinity() {
    let broker = Broker::default();
    let (first, mut first_rx) = host(&broker, protocol::COMMANDS);
    let (second, mut second_rx) = host(&broker, protocol::COMMANDS);
    let run = spawn(&broker, json!({"command":"list_tabs"}));
    let first_call = request(&mut first_rx).await;
    let second_call = request(&mut second_rx).await;
    let tabs: Vec<_> = (0..4096)
        .map(|index| {
            json!({
                "browserId":format!("1700000000000-{index:x}"), "title":"", "url":"about:blank"
            })
        })
        .collect();
    answer(
        &broker,
        &first,
        &first_call,
        &json!({"command":"list_tabs","tabs":tabs}),
    );
    answer(
        &broker,
        &second,
        &second_call,
        &json!({"command":"list_tabs",
        "tabs":[{"browserId":TAB,"title":"","url":"about:blank"}]}),
    );
    let result = run.await.unwrap();
    assert_eq!(result["error"]["code"], "browser_unknown_error");
    assert!(broker.0.lock().unwrap().affinity.is_empty());
    assert!(broker.0.lock().unwrap().pending.is_empty());
}

#[tokio::test]
async fn cancelling_fanout_clears_every_child_and_ignores_late_callbacks() {
    let broker = Broker::default();
    let (first, mut first_rx) = host(&broker, protocol::COMMANDS);
    let (second, mut second_rx) = host(&broker, protocol::COMMANDS);
    let run = spawn(&broker, json!({"command":"list_tabs"}));
    let first_call = request(&mut first_rx).await;
    let second_call = request(&mut second_rx).await;
    assert_eq!(broker.0.lock().unwrap().pending.len(), 2);
    run.abort();
    assert!(run.await.unwrap_err().is_cancelled());
    assert!(broker.0.lock().unwrap().pending.is_empty());
    for (lease, call) in [(&first, first_call), (&second, second_call)] {
        assert!(!broker.receive(
            call["requestId"].as_str().unwrap(),
            json!({"ok":true,"result":{"command":"list_tabs","tabs":[]}}),
            &[lease.id()].into_iter().collect()
        ));
    }
}

#[tokio::test]
async fn invalid_context_or_timeout_is_rejected_without_contacting_a_host() {
    let broker = Broker::default();
    let (_lease, mut receive) = host(&broker, protocol::COMMANDS);
    for context in [
        json!(null),
        json!([]),
        json!({"agentId":""}),
        json!({"cwd":7}),
        json!({"subscriptionId":"other"}),
        json!({"workspaceId":false}),
    ] {
        let result = broker
            .execute(
                json!({"command":"list_tabs"}),
                context,
                Duration::from_secs(1),
            )
            .await;
        assert_eq!(result["error"]["code"], "browser_unknown_error");
        assert_eq!(result["error"]["retryable"], false);
    }
    for timeout in [Duration::ZERO, Duration::from_millis(120_001)] {
        assert_eq!(
            broker
                .execute(json!({"command":"list_tabs"}), json!({}), timeout)
                .await["ok"],
            false
        );
    }
    assert!(receive.try_recv().is_err());
    assert!(broker.0.lock().unwrap().pending.is_empty());
}

#[tokio::test]
async fn disappearing_selected_host_never_receives_a_stale_request() {
    let broker = Broker::default();
    let (lease, mut receive) = host(&broker, &["list_tabs"]);
    let command = protocol::command(json!({"command":"list_tabs"})).unwrap();
    let selected = broker.select(&command).unwrap();
    drop(lease);
    let result = broker
        .send(
            &selected[0],
            "request",
            &command,
            &json!({}),
            Duration::from_secs(1),
            true,
        )
        .await;
    assert_eq!(result["error"]["code"], "browser_no_host");
    assert!(receive.try_recv().is_err());
    assert!(broker.0.lock().unwrap().pending.is_empty());
}
