use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::{Message, client::IntoClientRequest};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

use super::TOKEN;

pub(super) type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

pub(super) async fn connect(address: &str, capabilities: &[&str]) -> Socket {
    connect_as(address, capabilities, "process").await
}

pub(super) async fn connect_as(address: &str, capabilities: &[&str], client_id: &str) -> Socket {
    let mut request = format!("ws://{address}/v1/ws")
        .into_client_request()
        .unwrap();
    request
        .headers_mut()
        .insert("authorization", format!("Bearer {TOKEN}").parse().unwrap());
    let (mut socket, _) = tokio_tungstenite::connect_async(request).await.unwrap();
    socket.send(Message::Text(json!({"type":"hello","client_id":client_id,"protocol":{"major":1,"min_minor":0,"max_minor":0},"required_capabilities":capabilities}).to_string().into())).await.unwrap();
    assert_eq!(receive(&mut socket).await["type"], "server_info");
    socket
}

pub(super) async fn receive(socket: &mut Socket) -> Value {
    let message = tokio::time::timeout(Duration::from_secs(10), socket.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    serde_json::from_str(message.to_text().unwrap()).unwrap()
}

pub(super) async fn request(socket: &mut Socket, method: &str, params: Value) -> Value {
    socket
        .send(Message::Text(
            json!({"type":"request","request_id":"1","method":method,"params":params})
                .to_string()
                .into(),
        ))
        .await
        .unwrap();
    receive(socket).await
}
