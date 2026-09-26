//! Physical-connection and subscription contracts from Paseo owned-subscriptions tests.

use super::*;
use server_metadata::protocol::session::SessionEventKind;
use tokio_tungstenite::tungstenite::protocol::frame::Frame as WebSocketFrame;
use tokio_tungstenite::tungstenite::protocol::frame::coding::{Data, OpCode};

mod transport;

async fn connected(fixture: &Fixture, capabilities: Value) -> Socket {
    let mut socket = fixture.socket().await;
    let mut offer = hello();
    offer["capabilities"] = capabilities;
    send(&mut socket, offer).await;
    assert_eq!(receive(&mut socket).await["type"], "server_info");
    socket
}

async fn event_socket(fixture: &Fixture) -> Socket {
    connected(
        fixture,
        json!([
            "connection.ping",
            "session.events.set_subscription.request",
            "subscription.release.request",
            "session.heartbeat",
            "server.status.subscribe"
        ]),
    )
    .await
}

async fn observe_status(socket: &mut Socket) -> Value {
    let response = request(
        socket,
        "session.events.set_subscription.request",
        json!({"events":["status.server_info"]}),
    )
    .await;
    assert_eq!(response["type"], "response");
    assert!(response["result"]["subscriptionId"].is_string());
    response["result"].clone()
}

async fn assert_quiet_before_ping(socket: &mut Socket) {
    // The request is an ordered fence, avoiding a sleep-based negative assertion.
    let response = request(socket, "connection.ping", json!({"nonce":"delivery-fence"})).await;
    assert_eq!(
        response["type"], "response",
        "unexpected preceding delivery: {response}"
    );
    assert_eq!(response["result"]["nonce"], "delivery-fence");
}

fn publish(fixture: &Fixture, marker: u64) {
    fixture
        .api
        .shared
        .metadata
        .session_events
        .publish(SessionEventKind::ServerInfo, &json!({"marker":marker}));
}

#[tokio::test]
async fn identical_request_ids_in_one_logical_session_reply_only_to_the_source_socket() {
    let fixture = Fixture::start().await;
    let mut first = connected(&fixture, json!(["connection.ping"])).await;
    let mut second = connected(&fixture, json!(["connection.ping"])).await;
    for (socket, nonce) in [(&mut first, "first"), (&mut second, "second")] {
        send(socket, json!({"type":"request","request_id":"shared-id","method":"connection.ping","params":{"nonce":nonce}})).await;
    }
    assert_eq!(receive(&mut first).await["result"]["nonce"], "first");
    assert_eq!(receive(&mut second).await["result"]["nonce"], "second");
    assert_quiet_before_ping(&mut first).await;
    assert_quiet_before_ping(&mut second).await;
    fixture.stop().await;
}

#[tokio::test]
async fn protocol_rejection_is_source_only_and_does_not_poison_a_sibling_connection() {
    let fixture = Fixture::start().await;
    let mut first = connected(&fixture, json!(["connection.ping"])).await;
    let mut second = connected(&fixture, json!(["connection.ping"])).await;
    let response = request(&mut first, "missing.request", json!({})).await;
    assert_eq!(response["code"], "method_not_found");
    assert_eq!(response["request_id"], "r1");
    assert_quiet_before_ping(&mut first).await;
    assert_quiet_before_ping(&mut second).await;
    fixture.stop().await;
}

#[tokio::test]
async fn capability_negotiation_is_per_connection_even_for_identical_client_ids() {
    let fixture = Fixture::start().await;
    let mut broad = connected(&fixture, json!(["connection.ping", "server.info"])).await;
    let mut narrow = connected(&fixture, json!(["connection.ping"])).await;
    assert_eq!(
        request(&mut narrow, "server.info", Value::Null).await["code"],
        "unsupported_capability"
    );
    assert_eq!(
        request(&mut broad, "server.info", Value::Null).await["result"]["server_id"],
        "stable"
    );
    assert_quiet_before_ping(&mut narrow).await;
    fixture.stop().await;
}

#[tokio::test]
async fn identical_event_observers_get_distinct_ids_and_release_in_both_orders() {
    let fixture = Fixture::start().await;
    let mut socket = event_socket(&fixture).await;
    for release_first in [true, false] {
        let first = observe_status(&mut socket).await;
        let second = observe_status(&mut socket).await;
        assert_ne!(first["subscriptionId"], second["subscriptionId"]);
        publish(&fixture, 1);
        let delivered = [receive(&mut socket).await, receive(&mut socket).await];
        for lease in [&first, &second] {
            assert!(
                delivered
                    .iter()
                    .any(
                        |event| event["params"]["subscriptionId"] == lease["subscriptionId"]
                            && event["params"]["marker"] == 1
                    )
            );
        }
        let (released, remaining) = if release_first {
            (&first, &second)
        } else {
            (&second, &first)
        };
        request(
            &mut socket,
            "subscription.release.request",
            released.clone(),
        )
        .await;
        publish(&fixture, 2);
        let event = receive(&mut socket).await;
        assert_eq!(
            event["params"]["subscriptionId"],
            remaining["subscriptionId"]
        );
        assert_eq!(event["params"]["marker"], 2);
        request(
            &mut socket,
            "subscription.release.request",
            remaining.clone(),
        )
        .await;
        publish(&fixture, 3);
        assert_quiet_before_ping(&mut socket).await;
    }
    fixture.stop().await;
}

#[tokio::test]
async fn another_socket_cannot_release_or_receive_the_same_clients_event_observer() {
    let fixture = Fixture::start().await;
    let mut owner = event_socket(&fixture).await;
    let mut stranger = event_socket(&fixture).await;
    let lease = observe_status(&mut owner).await;
    request(&mut stranger, "subscription.release.request", lease.clone()).await;
    publish(&fixture, 7);
    assert_eq!(
        receive(&mut owner).await["params"]["subscriptionId"],
        lease["subscriptionId"]
    );
    assert_quiet_before_ping(&mut stranger).await;
    fixture.stop().await;
}

#[tokio::test]
async fn disconnect_drops_observers_and_a_reconnected_client_must_subscribe_again() {
    let fixture = Fixture::start().await;
    let mut original = event_socket(&fixture).await;
    let lease = observe_status(&mut original).await;
    original.close(None).await.unwrap();
    drop(original);
    let mut replacement = event_socket(&fixture).await;
    request(&mut replacement, "subscription.release.request", lease).await;
    publish(&fixture, 1);
    assert_quiet_before_ping(&mut replacement).await;
    let new_lease = observe_status(&mut replacement).await;
    publish(&fixture, 2);
    assert_eq!(
        receive(&mut replacement).await["params"]["subscriptionId"],
        new_lease["subscriptionId"]
    );
    fixture.stop().await;
}

#[tokio::test]
async fn event_category_subscription_does_not_enable_unrequested_categories() {
    let fixture = Fixture::start().await;
    let mut socket = event_socket(&fixture).await;
    observe_status(&mut socket).await;
    fixture.api.shared.metadata.session_events.publish(
        SessionEventKind::DaemonConfig,
        &json!({"private":"unrequested"}),
    );
    fixture.api.shared.metadata.session_events.publish(
        SessionEventKind::ProvidersSnapshot,
        &json!({"providers":[]}),
    );
    assert_quiet_before_ping(&mut socket).await;
    publish(&fixture, 1);
    assert_eq!(receive(&mut socket).await["method"], "status.server_info");
    fixture.stop().await;
}

#[tokio::test]
async fn duplicate_categories_produce_one_event_per_subscription() {
    let fixture = Fixture::start().await;
    let mut socket = event_socket(&fixture).await;
    let reply = request(
        &mut socket,
        "session.events.set_subscription.request",
        json!({"events":["status.server_info","status.server_info"]}),
    )
    .await;
    publish(&fixture, 1);
    assert_eq!(
        receive(&mut socket).await["params"]["subscriptionId"],
        reply["result"]["subscriptionId"]
    );
    assert_quiet_before_ping(&mut socket).await;
    fixture.stop().await;
}

#[tokio::test]
async fn an_empty_event_request_does_not_release_an_existing_observer() {
    let fixture = Fixture::start().await;
    let mut socket = event_socket(&fixture).await;
    let observed = observe_status(&mut socket).await;
    let empty = request(
        &mut socket,
        "session.events.set_subscription.request",
        json!({"events":[]}),
    )
    .await;
    assert_ne!(
        observed["subscriptionId"],
        empty["result"]["subscriptionId"]
    );
    publish(&fixture, 1);
    assert_eq!(
        receive(&mut socket).await["params"]["subscriptionId"],
        observed["subscriptionId"]
    );
    assert_quiet_before_ping(&mut socket).await;
    fixture.stop().await;
}

#[tokio::test]
async fn event_and_status_subscriptions_share_one_budget_and_release_restores_capacity() {
    let fixture = Fixture::start().await;
    let mut socket = event_socket(&fixture).await;
    let release = observe_status(&mut socket).await;
    for _ in 0..15 {
        assert!(request(&mut socket, "server.status.subscribe", Value::Null).await["result"]["subscription_id"].is_string());
        assert_eq!(receive(&mut socket).await["lifecycle"], "ready");
    }
    assert_eq!(
        request(
            &mut socket,
            "session.events.set_subscription.request",
            json!({"events":[]})
        )
        .await["code"],
        "resource_exhausted"
    );
    request(&mut socket, "subscription.release.request", release).await;
    assert_eq!(
        request(
            &mut socket,
            "session.events.set_subscription.request",
            json!({"events":[]})
        )
        .await["type"],
        "response"
    );
    fixture.stop().await;
}

#[tokio::test]
async fn malformed_event_subscription_does_not_replace_a_valid_observer() {
    let fixture = Fixture::start().await;
    let mut socket = event_socket(&fixture).await;
    let valid = observe_status(&mut socket).await;
    let bad = request(
        &mut socket,
        "session.events.set_subscription.request",
        json!({"events":["status.server_info"],"notifications":"yes"}),
    )
    .await;
    assert_eq!(bad["code"], "invalid_message");
    publish(&fixture, 1);
    assert_eq!(
        receive(&mut socket).await["params"]["subscriptionId"],
        valid["subscriptionId"]
    );
    fixture.stop().await;
}

#[tokio::test]
async fn heartbeat_has_no_acknowledgement_and_creates_no_event_demand() {
    let fixture = Fixture::start().await;
    let mut socket = event_socket(&fixture).await;
    send(&mut socket, json!({"type":"event","method":"session.heartbeat","params":{"deviceType":"web","appVisible":true,"lastActivityAt":"2026-09-26T00:00:00Z","focusedAgentId":"agent"}})).await;
    assert_quiet_before_ping(&mut socket).await;
    publish(&fixture, 1);
    assert_quiet_before_ping(&mut socket).await;
    fixture.stop().await;
}
