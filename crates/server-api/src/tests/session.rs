use super::*;

#[tokio::test]
async fn event_topics_require_installed_producers_and_shutdown_follows_the_subscription_response() {
    let fixture = Fixture::start().await;
    let mut socket = fixture.socket().await;
    let mut offer = hello();
    offer["capabilities"] = json!([
        "session.events.set_subscription.request",
        "session.heartbeat"
    ]);
    send(&mut socket, offer).await;
    receive(&mut socket).await;
    for event in [
        "agent_attention_required",
        "status.daemon_config_changed",
        "providers_snapshot_update",
    ] {
        assert_eq!(
            request(
                &mut socket,
                "session.events.set_subscription.request",
                json!({"events":[event]})
            )
            .await["code"],
            "unsupported_capability"
        );
    }
    assert_eq!(
        request(
            &mut socket,
            "session.events.set_subscription.request",
            json!({"events":false})
        )
        .await["code"],
        "invalid_message"
    );
    send(
        &mut socket,
        json!({"type":"event","method":"session.heartbeat","params":{}}),
    )
    .await;
    assert_eq!(receive(&mut socket).await["code"], "invalid_message");
    let subscribed = request(
        &mut socket,
        "session.events.set_subscription.request",
        json!({"events":["status.server_info"]}),
    )
    .await;
    assert_eq!(subscribed["type"], "response");
    fixture.api.begin_shutdown();
    fixture.api.begin_shutdown();
    let event = receive(&mut socket).await;
    assert_eq!(event["method"], "status.server_info");
    assert_eq!(
        event["params"]["subscriptionId"],
        subscribed["result"]["subscriptionId"]
    );
    assert_eq!(event["params"]["info"]["lifecycle"], "draining");
    fixture.stop().await;
}
