use std::net::SocketAddr;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use server_api::Api;
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::tungstenite::{Message, client::IntoClientRequest};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

use super::fixture::Fixture;

pub(super) const TOKEN: &str = "offline-project-test-token-at-least-32-bytes";
pub(super) type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

pub(super) async fn socket(address: SocketAddr, capabilities: &[&str]) -> Socket {
    let mut request = format!("ws://{address}/v1/ws")
        .into_client_request()
        .unwrap();
    request
        .headers_mut()
        .insert("authorization", format!("Bearer {TOKEN}").parse().unwrap());
    let (mut socket, _) = tokio_tungstenite::connect_async(request).await.unwrap();
    socket.send(Message::Text(json!({"type":"hello","client_id":"test","protocol":{"major":1,"min_minor":0,"max_minor":0},"required_capabilities":capabilities}).to_string().into())).await.unwrap();
    assert_eq!(receive(&mut socket).await["type"], "server_info");
    socket
}

async fn receive(socket: &mut Socket) -> Value {
    let frame = tokio::time::timeout(Duration::from_secs(10), socket.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    serde_json::from_str(frame.to_text().unwrap()).unwrap()
}

pub(super) async fn request(socket: &mut Socket, method: &str, params: Value) -> Value {
    socket
        .send(Message::Text(
            json!({"type":"request","request_id":"r","method":method,"params":params})
                .to_string()
                .into(),
        ))
        .await
        .unwrap();
    receive(socket).await
}

#[tokio::test]
async fn websocket_project_lifecycle_reconnect_and_validation() {
    let fixture = Fixture::new();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let api = Api::new(
        address,
        "server".to_owned(),
        "instance".to_owned(),
        TOKEN.into(),
        server_api::Services {
            projects: Some(fixture.application("state")),
            agents: None,
        },
    )
    .unwrap();
    let server_api = api.clone();
    let router = api.router();
    let server = tokio::spawn(async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(async move { server_api.wait_draining().await })
            .await
            .unwrap();
    });
    let mut limited = socket(address, &["connection.ping"]).await;
    assert_eq!(
        request(&mut limited, "project.list", json!({})).await["code"],
        "unsupported_capability"
    );
    let mut client = socket(address, server_protocol::project_lease::CAPABILITIES).await;
    for params in [
        json!({}),
        json!({"path":"relative","idempotency_key":"k"}),
        json!({"path":fixture.repo,"idempotency_key":"k","token":"must-not-be-accepted"}),
    ] {
        assert_eq!(
            request(&mut client, "project.open", params).await["code"],
            "invalid_message"
        );
    }
    let params = json!({"path":fixture.repo,"idempotency_key":"open"});
    let opened = request(&mut client, "project.open", params.clone()).await;
    assert_eq!(opened["type"], "response", "{opened}");
    let receipt = opened["result"].clone();
    let id = receipt["project_id"].clone();
    let project =
        request(&mut client, "project.get", json!({"project_id":id})).await["result"].clone();
    assert_eq!(project["owner_epoch"], 1);
    assert_eq!(
        request(&mut client, "project.list", json!({"limit":1})).await["result"]["next_after"],
        id
    );
    assert_eq!(
        request(&mut client, "project.list", json!({"after":id})).await["result"]["projects"],
        json!([])
    );
    for (method, params) in [
        ("project.list", json!({"limit":0})),
        ("project.list", json!({"after":"bad"})),
        ("project.get", json!({"project_id":"bad"})),
        ("project.close", json!({})),
    ] {
        assert_eq!(
            request(&mut client, method, params).await["code"],
            "invalid_message"
        );
    }
    drop(client);
    let mut reconnected = socket(address, server_protocol::project_lease::CAPABILITIES).await;
    assert_eq!(
        request(&mut reconnected, "project.open", params).await["result"],
        receipt
    );
    assert_eq!(
        request(
            &mut reconnected,
            "project.close",
            json!({"project_id":id,"owner_epoch":0,"idempotency_key":"close"})
        )
        .await["code"],
        "stale_owner"
    );
    assert_eq!(
        request(
            &mut reconnected,
            "project.close",
            json!({"project_id":id,"owner_epoch":1,"idempotency_key":"close"})
        )
        .await["result"]["project_id"],
        id
    );
    assert!(request(&mut reconnected,"project.get",json!({"project_id":id})).await["result"]["owner_epoch"].is_null());
    api.begin_shutdown();
    tokio::time::timeout(Duration::from_secs(10), async {
        server.await.unwrap();
        api.wait_closed().await;
    })
    .await
    .unwrap();
}
