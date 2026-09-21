use std::process::Command;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::{Message, client::IntoClientRequest};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

use super::{TOKEN, ready, start, terminate};

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

async fn connect(address: &str) -> Socket {
    let mut request = format!("ws://{address}/v1/ws")
        .into_client_request()
        .unwrap();
    request
        .headers_mut()
        .insert("authorization", format!("Bearer {TOKEN}").parse().unwrap());
    let (mut socket, _) = tokio_tungstenite::connect_async(request).await.unwrap();
    socket.send(Message::Text(json!({"type":"hello","client_id":"process","protocol":{"major":1,"min_minor":0,"max_minor":0},"required_capabilities":server_protocol::project::CAPABILITIES}).to_string().into())).await.unwrap();
    assert_eq!(receive(&mut socket).await["type"], "server_info");
    socket
}

async fn receive(socket: &mut Socket) -> Value {
    let message = tokio::time::timeout(Duration::from_secs(10), socket.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    serde_json::from_str(message.to_text().unwrap()).unwrap()
}

async fn request(socket: &mut Socket, method: &str, params: Value) -> Value {
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

#[tokio::test]
async fn binary_project_ownership_survives_catalogs_and_process_restarts() {
    let root = tempfile::tempdir().unwrap();
    let repo = root.path().join("repository");
    std::fs::create_dir(&repo).unwrap();
    for args in [
        vec!["init", "--quiet"],
        vec![
            "-c",
            "user.name=Server Test",
            "-c",
            "user.email=server@example.invalid",
            "commit",
            "--quiet",
            "--allow-empty",
            "-m",
            "initial",
        ],
    ] {
        assert!(
            Command::new("git")
                .args(args)
                .current_dir(&repo)
                .output()
                .unwrap()
                .status
                .success()
        );
    }
    let first_dir = root.path().join("first");
    let first_log = root.path().join("first.log");
    let second_dir = root.path().join("second");
    let second_log = root.path().join("second.log");
    let mut first = start(&first_dir, &first_log);
    let mut first_socket = connect(&ready(&mut first, &first_log).await).await;
    let params = json!({"path":repo,"idempotency_key":"open"});
    let opened = request(&mut first_socket, "project.open", params.clone()).await;
    assert_eq!(opened["type"], "response", "{opened}");
    let receipt = opened["result"].clone();
    let mut second = start(&second_dir, &second_log);
    let mut second_socket = connect(&ready(&mut second, &second_log).await).await;
    assert_eq!(
        request(&mut second_socket, "project.open", params.clone()).await["code"],
        "project_busy"
    );
    terminate(&mut first).await;
    assert_eq!(
        request(&mut second_socket, "project.open", params.clone()).await["result"]["project_id"],
        receipt["project_id"]
    );
    let mut restarted = start(&first_dir, &first_log);
    let mut restarted_socket = connect(&ready(&mut restarted, &first_log).await).await;
    assert_eq!(
        request(&mut restarted_socket, "project.open", params).await["result"],
        receipt
    );
    let summary = request(
        &mut restarted_socket,
        "project.get",
        json!({"project_id":receipt["project_id"]}),
    )
    .await;
    assert!(summary["result"]["owner_epoch"].is_null());
    terminate(&mut second).await;
    let acquired = request(
        &mut restarted_socket,
        "project.open",
        json!({"path":repo,"idempotency_key":"reacquire"}),
    )
    .await;
    assert_eq!(acquired["result"]["project_id"], receipt["project_id"]);
    terminate(&mut restarted).await;
}
