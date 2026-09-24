use super::*;

mod session;

use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::{Message, client::IntoClientRequest};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};

const TOKEN: &str = "offline-test-token-at-least-32-characters";
type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

struct Fixture {
    api: Api,
    address: SocketAddr,
    task: JoinHandle<()>,
}

impl Fixture {
    async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let api = Api::new(
            address,
            "stable".to_owned(),
            "instance".to_owned(),
            TOKEN.into(),
            crate::Services::default(),
        )
        .unwrap();
        let shutdown = api.clone();
        let router = api.router();
        let task = tokio::spawn(async move {
            axum::serve(listener, router)
                .with_graceful_shutdown(async move { shutdown.wait_draining().await })
                .await
                .unwrap();
        });
        Self { api, address, task }
    }

    fn url(&self, path: &str) -> String {
        format!("http://{}{path}", self.address)
    }

    async fn socket(&self) -> Socket {
        let mut request = format!("ws://{}/v1/ws", self.address)
            .into_client_request()
            .unwrap();
        request
            .headers_mut()
            .insert("authorization", format!("Bearer {TOKEN}").parse().unwrap());
        connect_async(request).await.unwrap().0
    }

    async fn stop(self) {
        self.api.begin_shutdown();
        tokio::time::timeout(Duration::from_secs(8), async {
            self.task.await.unwrap();
            self.api.wait_closed().await;
        })
        .await
        .unwrap();
    }
}

fn hello() -> Value {
    json!({"type":"hello", "client_id":"same-client", "protocol":{"major":1,"min_minor":0,"max_minor":4}, "capabilities":CAPABILITIES})
}

async fn send(socket: &mut Socket, value: Value) {
    socket
        .send(Message::Text(value.to_string().into()))
        .await
        .unwrap();
}

async fn receive(socket: &mut Socket) -> Value {
    let message = tokio::time::timeout(Duration::from_secs(3), socket.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    serde_json::from_str(message.to_text().unwrap()).unwrap()
}

async fn request(socket: &mut Socket, method: &str, params: Value) -> Value {
    send(
        socket,
        json!({"type":"request", "request_id":"r1", "method":method, "params":params}),
    )
    .await;
    receive(socket).await
}

#[test]
fn rejects_bad_config_and_redacts_debug() {
    for address in ["0.0.0.0:7316", "127.0.0.1:0"] {
        assert!(
            Api::new(
                address.parse().unwrap(),
                "s".to_owned(),
                "i".to_owned(),
                TOKEN.into(),
                crate::Services::default(),
            )
            .is_err()
        );
    }
    assert!(
        Api::new(
            "127.0.0.1:7316".parse().unwrap(),
            "s".to_owned(),
            "i".to_owned(),
            "short".into(),
            crate::Services::default(),
        )
        .is_err()
    );
    let api = Api::new(
        "127.0.0.1:7316".parse().unwrap(),
        "s".to_owned(),
        "i".to_owned(),
        TOKEN.into(),
        crate::Services::default(),
    )
    .unwrap();
    assert!(!format!("{api:?}").contains(TOKEN));
    let default_port = Api::new(
        "[::1]:80".parse().unwrap(),
        "s".to_owned(),
        "i".to_owned(),
        TOKEN.into(),
        crate::Services::default(),
    )
    .unwrap();
    let mut headers = axum::http::HeaderMap::new();
    headers.insert("host", "[::1]".parse().unwrap());
    headers.insert("origin", "http://localhost".parse().unwrap());
    assert!(auth::validate_source(&headers, &default_port.shared.authorities).is_ok());
}

#[tokio::test]
async fn http_authentication_origins_and_readiness() {
    let fixture = Fixture::start().await;
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    for path in ["/healthz", "/readyz"] {
        assert_eq!(
            client.get(fixture.url(path)).send().await.unwrap().status(),
            StatusCode::OK
        );
    }
    let info_url = fixture.url("/v1/server/info");
    assert_eq!(
        client.get(&info_url).send().await.unwrap().status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        client
            .get(&info_url)
            .bearer_auth("wrong")
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    for (header, value) in [
        ("host", "attacker.test"),
        ("origin", "http://attacker.test"),
    ] {
        assert_eq!(
            client
                .get(&info_url)
                .bearer_auth(TOKEN)
                .header(header, value)
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::FORBIDDEN
        );
    }
    assert_eq!(
        client
            .get(format!("{info_url}?token=forbidden"))
            .bearer_auth(TOKEN)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::BAD_REQUEST
    );
    let info: server_protocol::ServerInfo = client
        .get(&info_url)
        .bearer_auth(TOKEN)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(info.server_id, "stable");
    assert_eq!(info.lifecycle, Lifecycle::Ready);
    let expected: Vec<_> = CAPABILITIES
        .iter()
        .copied()
        .chain([
            "session.heartbeat",
            "session.events.set_subscription.request",
        ])
        .collect();
    assert_eq!(info.implemented_capabilities, expected);
    assert_eq!(info.capabilities.len(), 190);
    assert!(
        info.capabilities
            .contains(&"schedule.list.request".to_owned())
    );
    assert_eq!(
        client
            .get(fixture.url("/missing"))
            .bearer_auth(TOKEN)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
    // Exercise draining routes without closing the test listener yet.
    fixture.api.shared.cancellation.cancel();
    assert_eq!(
        ready(State(fixture.api.shared.clone()))
            .await
            .err()
            .unwrap()
            .0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(fixture.api.shared.info().lifecycle, Lifecycle::Draining);
    fixture.stop().await;
}

#[path = "tests/placeholders.rs"]
mod placeholders;

#[tokio::test]
async fn physical_connections_own_negotiation_and_subscriptions() {
    let fixture = Fixture::start().await;
    let (mut first, mut second) = (fixture.socket().await, fixture.socket().await);
    send(&mut first, hello()).await;
    send(&mut second, hello()).await;
    let first_info = receive(&mut first).await;
    let second_info = receive(&mut second).await;
    assert_ne!(first_info["connection_id"], second_info["connection_id"]);
    assert_eq!(first_info["info"]["protocol"], json!({"major":1,"minor":0}));
    assert_eq!(
        request(&mut first, "connection.ping", json!({"nonce":"n"})).await["result"]["nonce"],
        "n"
    );
    assert_eq!(
        request(&mut first, "server.info", Value::Null).await["result"]["server_id"],
        "stable"
    );
    let unknown = request(&mut first, "unknown.method", Value::Null).await;
    assert_eq!(unknown["code"], "method_not_found");
    assert_eq!(unknown["request_id"], "r1");
    assert_eq!(
        request(&mut first, "connection.ping", json!({})).await["code"],
        "invalid_message"
    );
    let subscription =
        request(&mut first, "server.status.subscribe", Value::Null).await["result"].clone();
    assert_eq!(receive(&mut first).await["lifecycle"], "ready");
    assert_eq!(
        request(
            &mut second,
            "server.status.unsubscribe",
            subscription.clone()
        )
        .await["code"],
        "subscription_not_found"
    );
    assert_eq!(
        request(
            &mut first,
            "server.status.unsubscribe",
            subscription.clone()
        )
        .await["result"]["unsubscribed"],
        true
    );
    assert_eq!(
        request(&mut first, "server.status.unsubscribe", subscription).await["code"],
        "subscription_not_found"
    );
    for _ in 0..16 {
        assert!(request(&mut first, "server.status.subscribe", Value::Null).await["result"]["subscription_id"].is_string());
        receive(&mut first).await;
    }
    assert_eq!(
        request(&mut first, "server.status.subscribe", Value::Null).await["code"],
        "resource_exhausted"
    );
    fixture.api.begin_shutdown();
    assert_eq!(receive(&mut first).await["lifecycle"], "draining");
    fixture.stop().await;
}

#[tokio::test]
async fn handshake_errors_are_closed_and_capabilities_are_enforced() {
    let fixture = Fixture::start().await;
    let mut bad_version = hello();
    bad_version["protocol"]["major"] = json!(2);
    let mut missing = hello();
    missing["required_capabilities"] = json!(["run.submit"]);
    for (message, code) in [
        (bad_version, "incompatible_version"),
        (missing, "unsupported_capability"),
        (
            json!({"type":"request","request_id":"early","method":"server.info"}),
            "invalid_message",
        ),
    ] {
        let mut socket = fixture.socket().await;
        send(&mut socket, message).await;
        assert_eq!(receive(&mut socket).await["code"], code);
        assert!(matches!(
            socket.next().await.unwrap().unwrap(),
            Message::Close(_)
        ));
    }
    let mut socket = fixture.socket().await;
    let mut minimal = hello();
    minimal["capabilities"] = json!([]);
    send(&mut socket, minimal).await;
    assert_eq!(
        receive(&mut socket).await["negotiated_capabilities"],
        json!([])
    );
    assert_eq!(
        request(&mut socket, "server.info", Value::Null).await["code"],
        "unsupported_capability"
    );
    send(&mut socket, hello()).await;
    assert_eq!(receive(&mut socket).await["code"], "invalid_message");
    fixture.stop().await;
}

#[tokio::test]
async fn rejects_oversized_frames_binary_and_malformed_messages() {
    let fixture = Fixture::start().await;
    for message in [
        Message::Text("{".into()),
        Message::Binary(vec![1, 2].into()),
    ] {
        let mut socket = fixture.socket().await;
        socket.send(message).await.unwrap();
        assert_eq!(receive(&mut socket).await["code"], "invalid_message");
    }
    let mut socket = fixture.socket().await;
    let mut oversized = hello();
    oversized["future"] = json!("x".repeat(server_protocol::MAX_MESSAGE_BYTES));
    let sent = socket
        .send(Message::Text(oversized.to_string().into()))
        .await;
    // Oversize is rejected by the WebSocket codec; never admitted as an RPC.
    if let Err(error) = sent {
        // A rejected header may close TCP before the client finishes writing its payload.
        assert!(
            matches!(error, tokio_tungstenite::tungstenite::Error::Io(error)
            if matches!(error.kind(), std::io::ErrorKind::BrokenPipe | std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::ConnectionAborted))
        );
    } else {
        let result = tokio::time::timeout(Duration::from_secs(3), socket.next())
            .await
            .unwrap();
        match result {
            Some(Ok(Message::Text(text))) => {
                let value: Value = serde_json::from_str(&text).unwrap();
                assert_eq!(value["code"], "invalid_message");
            }
            None | Some(Ok(Message::Close(_)) | Err(_)) => {}
            unexpected => panic!("unexpected oversized frame response: {unexpected:?}"),
        }
    }
    fixture.stop().await;
}

#[tokio::test]
async fn unauthenticated_upgrade_and_connection_budget_are_rejected() {
    let fixture = Fixture::start().await;
    let request = format!("ws://{}/v1/ws", fixture.address)
        .into_client_request()
        .unwrap();
    let error = connect_async(request).await.unwrap_err();
    assert!(
        matches!(error, tokio_tungstenite::tungstenite::Error::Http(response) if response.status() == StatusCode::UNAUTHORIZED)
    );
    let mut sockets = Vec::new();
    for _ in 0..server_protocol::MAX_CONNECTIONS {
        sockets.push(fixture.socket().await);
    }
    let mut request = format!("ws://{}/v1/ws", fixture.address)
        .into_client_request()
        .unwrap();
    request
        .headers_mut()
        .insert("authorization", format!("Bearer {TOKEN}").parse().unwrap());
    let error = connect_async(request).await.unwrap_err();
    assert!(
        matches!(error, tokio_tungstenite::tungstenite::Error::Http(response) if response.status() == StatusCode::TOO_MANY_REQUESTS)
    );
    fixture.stop().await;
    drop(sockets);
}

#[tokio::test]
async fn hello_has_a_deadline_and_idle_connections_drain() {
    let fixture = Fixture::start().await;
    let mut silent = fixture.socket().await;
    let message = tokio::time::timeout(Duration::from_secs(12), silent.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(message.to_text().unwrap().contains("invalid_message"));
    let _idle = fixture.socket().await;
    fixture.stop().await;
}
