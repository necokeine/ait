use super::*;
use server_model::outbound::{Frame, Queued};
const TAB: &str = "11111111-1111-4111-8111-111111111111";
fn host(broker: &Broker, commands: &[&str]) -> (Registration, tokio::sync::mpsc::Receiver<Queued>) {
    let (outbound, receive) = Outbound::new();
    (
        broker
            .register(
                Register {
                    host_kind: "desktop".into(),
                    supported_commands: commands.iter().map(|s| (*s).into()).collect(),
                },
                outbound,
            )
            .unwrap(),
        receive,
    )
}
async fn request(receiver: &mut tokio::sync::mpsc::Receiver<Queued>) -> Value {
    let queued = receiver.recv().await.unwrap();
    let Frame::Text(text) = queued.message else {
        panic!("text event expected")
    };
    serde_json::from_str::<Value>(&text).unwrap()["params"].clone()
}
fn spawn(broker: &Broker, command: Value) -> tokio::task::JoinHandle<Value> {
    let broker = broker.clone();
    tokio::spawn(async move {
        broker
            .execute(command, json!({}), Duration::from_secs(1))
            .await
    })
}
fn answer(broker: &Broker, host: &Registration, request: &Value, result: &Value) {
    assert!(broker.receive(
        request["requestId"].as_str().unwrap(),
        json!({"ok":true,"result":result}),
        &[host.id()].into_iter().collect()
    ));
}
#[tokio::test]
async fn newest_host_then_tab_affinity_and_stranded_owner() {
    let broker = Broker::default();
    let (first, mut rx1) = host(&broker, protocol::COMMANDS);
    let (second, mut rx2) = host(&broker, protocol::COMMANDS);
    let run = spawn(&broker, json!({"command":"new_tab"}));
    let call = request(&mut rx2).await;
    assert_eq!(call["subscriptionId"], second.id());
    answer(
        &broker,
        &second,
        &call,
        &json!({"command":"new_tab","browserId":TAB,"workspaceId":"w","url":"about:blank"}),
    );
    assert_eq!(run.await.unwrap()["ok"], true);
    assert!(rx1.try_recv().is_err());
    let run = spawn(
        &broker,
        json!({"command":"reload","args":{"browserId":TAB}}),
    );
    let call = request(&mut rx2).await;
    assert!(!broker.receive(
        call["requestId"].as_str().unwrap(),
        json!({"ok":true}),
        &[first.id()].into_iter().collect()
    ));
    answer(
        &broker,
        &second,
        &call,
        &json!({"command":"reload","browserId":TAB}),
    );
    assert_eq!(run.await.unwrap()["ok"], true);
    drop(second);
    assert_eq!(
        spawn(
            &broker,
            json!({"command":"reload","args":{"browserId":TAB}})
        )
        .await
        .unwrap()["error"]["code"],
        "browser_no_host"
    );
}
#[tokio::test]
async fn fanout_aggregates_in_registration_order_and_learns_affinity() {
    let broker = Broker::default();
    let (a, mut arx) = host(&broker, protocol::COMMANDS);
    let (b, mut brx) = host(&broker, protocol::COMMANDS);
    assert_eq!(
        spawn(
            &broker,
            json!({"command":"reload","args":{"browserId":TAB}})
        )
        .await
        .unwrap()["error"]["code"],
        "browser_tab_not_found"
    );
    let run = spawn(&broker, json!({"command":"list_tabs"}));
    let call_a = request(&mut arx).await;
    let call_b = request(&mut brx).await;
    answer(
        &broker,
        &b,
        &call_b,
        &json!({"command":"list_tabs","tabs":[]}),
    );
    answer(
        &broker,
        &a,
        &call_a,
        &json!({"command":"list_tabs","tabs":[{"browserId":TAB,"url":"https://example.com","title":"Page"}]}),
    );
    let result = run.await.unwrap();
    assert_eq!(result["result"]["tabs"][0]["isActive"], false);
    let run = spawn(
        &broker,
        json!({"command":"close_tab","args":{"browserId":TAB}}),
    );
    let call = request(&mut arx).await;
    answer(
        &broker,
        &a,
        &call,
        &json!({"command":"close_tab","browserId":TAB}),
    );
    assert_eq!(run.await.unwrap()["ok"], true);
    assert!(broker.0.lock().unwrap().affinity.is_empty());
}
#[tokio::test]
async fn unsupported_timeout_disconnect_and_malformed_callbacks_settle_and_cleanup() {
    let broker = Broker::default();
    assert_eq!(
        spawn(&broker, json!({"command":"list_tabs"}))
            .await
            .unwrap()["error"]["code"],
        "browser_no_host"
    );
    let (lease, mut rx) = host(&broker, &["list_tabs"]);
    assert_eq!(
        spawn(&broker, json!({"command":"new_tab"})).await.unwrap()["error"]["code"],
        "browser_unsupported"
    );
    let result = broker
        .execute(
            json!({"command":"list_tabs"}),
            json!({}),
            Duration::from_millis(1),
        )
        .await;
    assert_eq!(result["error"]["code"], "browser_timeout");
    assert!(broker.0.lock().unwrap().pending.is_empty());
    let _ = request(&mut rx).await;
    let run = spawn(&broker, json!({"command":"list_tabs"}));
    let call = request(&mut rx).await;
    assert!(broker.receive(
        call["requestId"].as_str().unwrap(),
        json!({"ok":true,"result":{}}),
        &[lease.id()].into_iter().collect()
    ));
    assert_eq!(run.await.unwrap()["error"]["code"], "browser_unknown_error");
    let run = spawn(&broker, json!({"command":"list_tabs"}));
    let _ = request(&mut rx).await;
    drop(lease);
    assert_eq!(run.await.unwrap()["error"]["code"], "browser_no_host");
    assert!(broker.0.lock().unwrap().pending.is_empty());
}
#[tokio::test]
async fn caller_cancel_and_send_failure_release_pending_entries() {
    let broker = Broker::default();
    let (_lease, mut rx) = host(&broker, protocol::COMMANDS);
    let run = spawn(&broker, json!({"command":"list_tabs"}));
    let _ = request(&mut rx).await;
    run.abort();
    assert!(run.await.is_err());
    assert!(broker.0.lock().unwrap().pending.is_empty());
    drop(rx);
    assert_eq!(
        spawn(&broker, json!({"command":"list_tabs"}))
            .await
            .unwrap()["ok"],
        false
    );
    assert!(broker.0.lock().unwrap().pending.is_empty());
}
#[tokio::test]
async fn failed_fanout_does_not_learn_partial_affinity() {
    let broker = Broker::default();
    let (a, mut arx) = host(&broker, protocol::COMMANDS);
    let (b, mut brx) = host(&broker, protocol::COMMANDS);
    let run = spawn(&broker, json!({"command":"list_tabs"}));
    let ca = request(&mut arx).await;
    let cb = request(&mut brx).await;
    answer(
        &broker,
        &a,
        &ca,
        &json!({"command":"list_tabs","tabs":[{"browserId":TAB,"url":"","title":""}]}),
    );
    assert!(broker.receive(
        cb["requestId"].as_str().unwrap(),
        json!({"ok":false,"error":{"code":"browser_denied","message":"No"}}),
        &[b.id()].into_iter().collect()
    ));
    assert_eq!(run.await.unwrap()["ok"], false);
    assert!(broker.0.lock().unwrap().affinity.is_empty());
}
#[test]
fn registration_rejects_unknown_commands() {
    let broker = Broker::default();
    let (outbound, _) = Outbound::new();
    assert!(
        broker
            .register(
                Register {
                    host_kind: "desktop".into(),
                    supported_commands: vec!["unknown".into()]
                },
                outbound
            )
            .is_err()
    );
}

mod limits;
mod routing;
