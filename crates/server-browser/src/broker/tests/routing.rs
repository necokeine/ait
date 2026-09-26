//! Behavioral counterparts of Paseo browser-tools/broker.test.ts.
use super::*;

const OTHER_TAB: &str = "22222222-2222-4222-8222-222222222222";

#[tokio::test]
async fn explicit_host_failure_preserves_code_message_and_retryability() {
    let broker = Broker::default();
    let (lease, mut receive) = host(&broker, protocol::COMMANDS);
    let run = spawn(
        &broker,
        json!({"command":"snapshot","args":{"browserId":TAB}}),
    );
    let call = request(&mut receive).await;
    let error =
        json!({"code":"browser_stale_ref","message":"Refresh the snapshot","retryable":true});
    assert!(broker.receive(
        call["requestId"].as_str().unwrap(),
        json!({"ok":false,"error":error}),
        &[lease.id()].into_iter().collect()
    ));
    let result = run.await.unwrap();
    assert_eq!(result["error"], error);
    assert_eq!(result["requestId"], call["requestId"]);
    assert!(broker.0.lock().unwrap().affinity.is_empty());
}

#[tokio::test]
async fn snapshot_for_unknown_tab_routes_to_the_only_host_and_forwards_context() {
    let broker = Broker::default();
    let (lease, mut receive) = host(&broker, protocol::COMMANDS);
    let runner = broker.clone();
    let run = tokio::spawn(async move {
        runner
            .execute(
                json!({"command":"snapshot","args":{"browserId":TAB}}),
                json!({"agentId":"agent","cwd":"/repo","workspaceId":"workspace"}),
                Duration::from_secs(5),
            )
            .await
    });
    let call = request(&mut receive).await;
    assert_eq!(call["agentId"], "agent");
    assert_eq!(call["cwd"], "/repo");
    assert_eq!(call["workspaceId"], "workspace");
    answer(
        &broker,
        &lease,
        &call,
        &json!({
            "command":"snapshot","browserId":TAB,"url":"about:blank","title":"",
            "format":"aria-yaml","snapshot":"- document","truncated":false,
            "stats":{"nodeCount":1,"refCount":0,"textLength":10}
        }),
    );
    assert_eq!(run.await.unwrap()["ok"], true);
    assert_eq!(broker.0.lock().unwrap().affinity[TAB], lease.id());
}

#[tokio::test]
async fn mismatched_reply_request_id_settles_only_its_matched_pending_request() {
    let broker = Broker::default();
    let (lease, mut receive) = host(&broker, protocol::COMMANDS);
    let first = spawn(&broker, json!({"command":"list_tabs"}));
    let first_call = request(&mut receive).await;
    let second = spawn(&broker, json!({"command":"list_tabs"}));
    let second_call = request(&mut receive).await;
    assert!(broker.receive(first_call["requestId"].as_str().unwrap(),
        json!({"requestId":second_call["requestId"],"ok":true,"result":{"command":"list_tabs","tabs":[]}}),
        &[lease.id()].into_iter().collect()));
    assert_eq!(
        first.await.unwrap()["error"]["code"],
        "browser_unknown_error"
    );
    assert!(!second.is_finished());
    answer(
        &broker,
        &lease,
        &second_call,
        &json!({"command":"list_tabs","tabs":[]}),
    );
    assert_eq!(second.await.unwrap()["ok"], true);
}

#[tokio::test]
async fn mismatched_reply_browser_id_does_not_steal_tab_affinity() {
    let broker = Broker::default();
    let (lease, mut receive) = host(&broker, protocol::COMMANDS);
    let run = spawn(
        &broker,
        json!({"command":"reload","args":{"browserId":TAB}}),
    );
    let call = request(&mut receive).await;
    answer(
        &broker,
        &lease,
        &call,
        &json!({"command":"reload","browserId":OTHER_TAB}),
    );
    assert_eq!(run.await.unwrap()["error"]["code"], "browser_unknown_error");
    assert!(broker.0.lock().unwrap().affinity.is_empty());
}

#[tokio::test]
async fn unknown_and_duplicate_callbacks_cannot_complete_another_call() {
    let broker = Broker::default();
    let (lease, mut receive) = host(&broker, protocol::COMMANDS);
    let run = spawn(&broker, json!({"command":"list_tabs"}));
    let call = request(&mut receive).await;
    let payload = json!({"ok":true,"result":{"command":"list_tabs","tabs":[]}});
    let owners = &[lease.id()].into_iter().collect();
    assert!(!broker.receive("unknown", payload.clone(), owners));
    assert_eq!(broker.0.lock().unwrap().pending.len(), 1);
    assert!(broker.receive(call["requestId"].as_str().unwrap(), payload.clone(), owners));
    assert!(!broker.receive(call["requestId"].as_str().unwrap(), payload, owners));
    assert_eq!(run.await.unwrap()["ok"], true);
}

#[tokio::test]
async fn callback_after_timeout_does_not_restore_tab_affinity() {
    let broker = Broker::default();
    let (lease, mut receive) = host(&broker, protocol::COMMANDS);
    let result = broker
        .execute(
            json!({"command":"new_tab"}),
            json!({}),
            Duration::from_millis(1),
        )
        .await;
    let call = request(&mut receive).await;
    assert_eq!(result["error"]["code"], "browser_timeout");
    assert!(!broker.receive(call["requestId"].as_str().unwrap(), json!({"ok":true,
        "result":{"command":"new_tab","browserId":TAB,"workspaceId":"workspace","url":"about:blank"}}),
        &[lease.id()].into_iter().collect()));
    assert!(broker.0.lock().unwrap().affinity.is_empty());
}

#[tokio::test]
async fn reconnect_reclaims_stranded_tab_only_after_successful_listing() {
    let broker = Broker::default();
    let (old, mut old_rx) = host(&broker, protocol::COMMANDS);
    let run = spawn(
        &broker,
        json!({"command":"reload","args":{"browserId":TAB}}),
    );
    let call = request(&mut old_rx).await;
    answer(
        &broker,
        &old,
        &call,
        &json!({"command":"reload","browserId":TAB}),
    );
    assert_eq!(run.await.unwrap()["ok"], true);
    drop(old);
    let (new, mut new_rx) = host(&broker, protocol::COMMANDS);
    assert_eq!(
        spawn(
            &broker,
            json!({"command":"reload","args":{"browserId":TAB}})
        )
        .await
        .unwrap()["error"]["code"],
        "browser_no_host"
    );
    let run = spawn(&broker, json!({"command":"list_tabs"}));
    let call = request(&mut new_rx).await;
    answer(
        &broker,
        &new,
        &call,
        &json!({"command":"list_tabs",
        "tabs":[{"browserId":TAB,"url":"about:blank","title":""}]}),
    );
    assert_eq!(run.await.unwrap()["ok"], true);
    assert!(!broker.0.lock().unwrap().stranded.contains_key(TAB));
    let run = spawn(
        &broker,
        json!({"command":"reload","args":{"browserId":TAB}}),
    );
    let call = request(&mut new_rx).await;
    answer(
        &broker,
        &new,
        &call,
        &json!({"command":"reload","browserId":TAB}),
    );
    assert_eq!(run.await.unwrap()["ok"], true);
}

#[tokio::test]
async fn unsupported_host_in_fanout_rejects_before_any_delivery() {
    let broker = Broker::default();
    let (_first, mut first_rx) = host(&broker, protocol::COMMANDS);
    let (_second, mut second_rx) = host(&broker, &["new_tab"]);
    let result = spawn(&broker, json!({"command":"list_tabs"}))
        .await
        .unwrap();
    assert_eq!(result["error"]["code"], "browser_unsupported");
    assert!(first_rx.try_recv().is_err());
    assert!(second_rx.try_recv().is_err());
    assert!(broker.0.lock().unwrap().pending.is_empty());
}

#[tokio::test]
async fn dropping_one_lease_settles_only_its_own_requests() {
    let broker = Broker::default();
    let (first, mut first_rx) = host(&broker, protocol::COMMANDS);
    let (second, mut second_rx) = host(&broker, protocol::COMMANDS);
    let run = spawn(&broker, json!({"command":"list_tabs"}));
    let _ = request(&mut first_rx).await;
    let call = request(&mut second_rx).await;
    drop(first);
    assert_eq!(broker.0.lock().unwrap().pending.len(), 1);
    answer(
        &broker,
        &second,
        &call,
        &json!({"command":"list_tabs","tabs":[]}),
    );
    assert_eq!(run.await.unwrap()["error"]["code"], "browser_no_host");
    let followup = spawn(&broker, json!({"command":"list_tabs"}));
    let call = request(&mut second_rx).await;
    answer(
        &broker,
        &second,
        &call,
        &json!({"command":"list_tabs","tabs":[]}),
    );
    assert_eq!(followup.await.unwrap()["ok"], true);
}
