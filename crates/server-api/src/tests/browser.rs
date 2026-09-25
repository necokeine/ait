use super::*;
async fn browser_socket(fixture: &Fixture) -> Socket {
    let mut socket = fixture.socket().await;
    let mut offer = hello();
    offer["capabilities"] = json!([
        "browser.host.register.request",
        "browser.automation.execute.response",
        "subscription.release.request",
        "connection.ping"
    ]);
    send(&mut socket, offer).await;
    receive(&mut socket).await;
    socket
}
#[tokio::test]
async fn browser_callbacks_are_connection_owned_and_release_settles_pending_execution() {
    let broker = server_browser::broker::Broker::default();
    let fixture = Fixture::with_services(Services {
        browser: Some(broker.clone()),
        ..Services::default()
    })
    .await;
    let mut owner = browser_socket(&fixture).await;
    let mut stranger = browser_socket(&fixture).await;
    let lease = request(
        &mut owner,
        "browser.host.register.request",
        json!({"hostKind":"desktop","supportedCommands":["list_tabs"]}),
    )
    .await["result"]["subscriptionId"]
        .clone();
    assert!(lease.is_string());
    let active = broker.clone();
    let run = tokio::spawn(async move {
        active
            .execute(
                json!({"command":"list_tabs"}),
                json!({}),
                Duration::from_secs(2),
            )
            .await
    });
    let event = receive(&mut owner).await;
    assert_eq!(event["method"], "browser.automation.execute.request");
    assert_eq!(event["params"]["subscriptionId"], lease);
    let callback = json!({"type":"response","method":"browser.automation.execute.response","request_id":event["params"]["requestId"],"params":{"ok":true,"result":{"command":"list_tabs","tabs":[]}}});
    send(&mut stranger, callback.clone()).await;
    let ping = request(
        &mut stranger,
        "connection.ping",
        json!({"nonce":"ordering"}),
    )
    .await;
    assert_eq!(ping["result"]["nonce"], "ordering");
    assert!(!run.is_finished());
    send(&mut owner, callback).await;
    assert_eq!(run.await.unwrap()["ok"], true);
    let active = broker.clone();
    let run = tokio::spawn(async move {
        active
            .execute(
                json!({"command":"list_tabs"}),
                json!({}),
                Duration::from_secs(2),
            )
            .await
    });
    receive(&mut owner).await;
    request(
        &mut stranger,
        "subscription.release.request",
        json!({"subscriptionId":lease}),
    )
    .await;
    assert!(!run.is_finished());
    request(
        &mut owner,
        "subscription.release.request",
        json!({"subscriptionId":lease}),
    )
    .await;
    assert_eq!(run.await.unwrap()["error"]["code"], "browser_no_host");
    fixture.stop().await;
}
#[tokio::test]
async fn host_disconnect_settles_pending_and_registration_consumes_subscription_budget() {
    let broker = server_browser::broker::Broker::default();
    let fixture = Fixture::with_services(Services {
        browser: Some(broker.clone()),
        ..Services::default()
    })
    .await;
    let mut socket = browser_socket(&fixture).await;
    for _ in 0..16 {
        assert_eq!(
            request(
                &mut socket,
                "browser.host.register.request",
                json!({"hostKind":"desktop","supportedCommands":["list_tabs"]})
            )
            .await["type"],
            "response"
        );
    }
    assert_eq!(
        request(
            &mut socket,
            "browser.host.register.request",
            json!({"hostKind":"desktop","supportedCommands":["list_tabs"]})
        )
        .await["code"],
        "resource_exhausted"
    );
    let run = tokio::spawn(async move {
        broker
            .execute(
                json!({"command":"list_tabs"}),
                json!({}),
                Duration::from_secs(2),
            )
            .await
    });
    receive(&mut socket).await;
    socket.close(None).await.unwrap();
    drop(socket);
    assert_eq!(run.await.unwrap()["error"]["code"], "browser_no_host");
    fixture.stop().await;
}
