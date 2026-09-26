use std::fs;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use server_filesystem::protocol::file_transfer::{self, FileBegin, FileFrame};
use tokio_tungstenite::tungstenite::Message;

use super::transport::{Socket, connect, receive, request};
use super::{ready, start, terminate};

#[path = "files/paseo.rs"]
mod paseo;

#[tokio::test]
async fn filesystem_requests_preserve_edits_and_connection_owned_versions() {
    let temp = tempfile::tempdir().unwrap();
    let cwd = temp.path().join("workspace");
    fs::create_dir(&cwd).unwrap();
    let log = temp.path().join("log");
    let mut process = start(&temp.path().join("state"), &log);
    let address = ready(&mut process, &log).await;
    let mut methods = server_filesystem::protocol::files::CAPABILITIES.to_vec();
    methods.push("subscription.release.request");
    let mut socket = connect(&address, &methods).await;
    let created = request(
        &mut socket,
        "fs.entry.create.request",
        json!({"cwd":cwd,"parentPath":".","name":"file.txt","kind":"file"}),
    )
    .await;
    assert_eq!(created["result"]["success"], true, "{created}");
    let listed = request(
        &mut socket,
        "fs.explorer.request",
        json!({"cwd":cwd,"mode":"list"}),
    )
    .await;
    assert_eq!(
        listed["result"]["directory"]["entries"][0]["name"],
        "file.txt"
    );
    let read = request(
        &mut socket,
        "fs.explorer.request",
        json!({"cwd":cwd,"path":"file.txt","mode":"file"}),
    )
    .await;
    let revision = read["result"]["file"]["revision"].clone();
    let written = request(&mut socket, "fs.file.write.request", json!({"cwd":cwd,"path":"file.txt","content":"hello","expectedModifiedAt":"ignored","expectedRevision":revision})).await;
    assert_eq!(
        written["result"]["result"]["status"], "written",
        "{written}"
    );
    let conflict = request(&mut socket, "fs.file.write.request", json!({"cwd":cwd,"path":"file.txt","content":"stale","expectedModifiedAt":"ignored","expectedRevision":revision})).await;
    assert_eq!(conflict["result"]["result"]["status"], "conflict");
    let copied = request(
        &mut socket,
        "fs.entry.duplicate.request",
        json!({"cwd":cwd,"path":"file.txt"}),
    )
    .await;
    assert_eq!(copied["result"]["duplicatedPath"], "file copy.txt");
    let renamed = request(
        &mut socket,
        "fs.entry.rename.request",
        json!({"cwd":cwd,"path":"file copy.txt","name":"copy.txt"}),
    )
    .await;
    assert_eq!(renamed["result"]["renamedPath"], "copy.txt");
    let found = request(
        &mut socket,
        "directory.suggestions.request",
        json!({"cwd":cwd,"query":"copy","includeFiles":true}),
    )
    .await;
    assert_eq!(found["result"]["entries"][0]["path"], "copy.txt");
    check_download(&mut socket, &address, &cwd).await;
    check_subscription(&mut socket, &address, &cwd, &methods).await;
    let deleted = request(
        &mut socket,
        "fs.entry.delete.request",
        json!({"cwd":cwd,"path":"copy.txt"}),
    )
    .await;
    assert_eq!(deleted["result"]["success"], true);
    let denied = request(
        &mut socket,
        "fs.entry.delete.request",
        json!({"cwd":cwd,"path":"."}),
    )
    .await;
    assert_eq!(denied["result"]["success"], false);
    socket.close(None).await.unwrap();
    terminate(&mut process).await;
}

#[tokio::test]
async fn binary_preview_streams_bounded_chunks_and_honors_max_bytes() {
    let temp = tempfile::tempdir().unwrap();
    let cwd = temp.path().join("workspace");
    fs::create_dir(&cwd).unwrap();
    let bytes = vec![b'x'; 700_000];
    fs::write(cwd.join("large.txt"), &bytes).unwrap();
    let log = temp.path().join("log");
    let mut process = start(&temp.path().join("state"), &log);
    let address = ready(&mut process, &log).await;
    let mut socket = connect(&address, server_filesystem::protocol::files::CAPABILITIES).await;
    send_request(
        &mut socket,
        "preview",
        "fs.explorer.request",
        json!({"cwd":cwd,"path":"large.txt","mode":"file","acceptBinary":true}),
    )
    .await;
    let mut received = Vec::new();
    let mut began = false;
    loop {
        let message = tokio::time::timeout(Duration::from_secs(10), socket.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let Message::Binary(bytes) = message else {
            panic!("expected binary transfer, got {message:?}");
        };
        let (id, frame) = file_transfer::decode(&bytes).unwrap();
        assert_eq!(id, "preview");
        match frame {
            FileFrame::Begin(metadata) => {
                assert!(!began);
                began = true;
                assert_eq!(metadata.size, 700_000);
            }
            FileFrame::Chunk(bytes) => {
                assert!(began);
                received.extend(bytes);
            }
            FileFrame::End => break,
        }
    }
    assert_eq!(received, bytes);
    let limited = request(
        &mut socket,
        "fs.explorer.request",
        json!({"cwd":cwd,"path":"large.txt","mode":"file","acceptBinary":true,"maxBytes":10}),
    )
    .await;
    assert_eq!(limited["result"]["error"], "File is too large to display");
    let inline = request(
        &mut socket,
        "fs.explorer.request",
        json!({"cwd":cwd,"path":"large.txt","mode":"file"}),
    )
    .await;
    assert_eq!(inline["result"]["error"], "File is too large to display");
    socket.close(None).await.unwrap();
    terminate(&mut process).await;
}

#[tokio::test]
async fn upload_frames_are_connection_owned_and_failures_remove_partial_files() {
    let temp = tempfile::tempdir().unwrap();
    let log = temp.path().join("log");
    let state = temp.path().join("state");
    let mut process = start(&state, &log);
    let address = ready(&mut process, &log).await;
    let mut first = connect(&address, server_filesystem::protocol::files::CAPABILITIES).await;
    let mut other = connect(&address, server_filesystem::protocol::files::CAPABILITIES).await;
    let request_params =
        json!({"fileName":"../file?.txt","mimeType":"text/plain","size":3,"modifiedAt":"now"});
    send_request(
        &mut first,
        "upload",
        "file.upload.request",
        request_params.clone(),
    )
    .await;
    send_frame(&mut other, "upload", FileFrame::Chunk(b"bad".to_vec())).await;
    assert_eq!(receive(&mut other).await["code"], "invalid_message");
    send_frame(&mut first, "upload", begin(3)).await;
    send_frame(&mut first, "upload", FileFrame::Chunk(b"abc".to_vec())).await;
    send_frame(&mut first, "upload", FileFrame::End).await;
    let result = receive(&mut first).await;
    assert_eq!(
        result["result"]["file"]["type"], "uploaded_file",
        "{result}"
    );
    assert_eq!(result["result"]["file"]["fileName"], "file_.txt");
    assert_eq!(
        fs::read(result["result"]["file"]["path"].as_str().unwrap()).unwrap(),
        b"abc"
    );
    send_request(
        &mut first,
        "bad",
        "file.upload.request",
        request_params.clone(),
    )
    .await;
    send_frame(&mut first, "bad", begin(3)).await;
    send_frame(&mut first, "bad", FileFrame::Chunk(b"too long".to_vec())).await;
    assert!(
        receive(&mut first).await["result"]["error"]
            .as_str()
            .unwrap()
            .contains("exceeded")
    );
    send_request(
        &mut first,
        "early",
        "file.upload.request",
        request_params.clone(),
    )
    .await;
    send_frame(&mut first, "early", FileFrame::End).await;
    assert!(
        receive(&mut first).await["result"]["error"]
            .as_str()
            .unwrap()
            .contains("before file begin")
    );
    send_request(&mut first, "partial", "file.upload.request", request_params).await;
    send_frame(&mut first, "partial", begin(3)).await;
    send_frame(&mut first, "partial", FileFrame::Chunk(b"a".to_vec())).await;
    // A following response is an ordering barrier for the accepted upload chunks.
    request(
        &mut first,
        "fs.file.unsubscribe.request",
        json!({"subscriptionId":"unused"}),
    )
    .await;
    assert_eq!(fs::read_dir(state.join("uploads")).unwrap().count(), 2);
    first.close(None).await.unwrap();
    for _ in 0..100 {
        if fs::read_dir(state.join("uploads")).unwrap().count() == 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(fs::read_dir(state.join("uploads")).unwrap().count(), 1);
    other.close(None).await.unwrap();
    terminate(&mut process).await;
}

fn begin(size: u64) -> FileFrame {
    FileFrame::Begin(FileBegin {
        mime: "text/plain".to_owned(),
        size,
        encoding: "binary".to_owned(),
        modified_at: "now".to_owned(),
        revision: None,
        file_name: None,
    })
}

async fn send_frame(socket: &mut Socket, id: &str, frame: FileFrame) {
    socket
        .send(Message::Binary(
            file_transfer::encode(id, &frame).unwrap().into(),
        ))
        .await
        .unwrap();
}

async fn check_download(socket: &mut Socket, address: &str, cwd: &std::path::Path) {
    let token = request(
        socket,
        "fs.file.download_token.request",
        json!({"cwd":cwd,"path":"file.txt"}),
    )
    .await;
    let url = format!(
        "http://{address}/api/files/download?token={}",
        token["result"]["token"].as_str().unwrap()
    );
    let client = reqwest::Client::new();
    let download = client.get(&url).send().await.unwrap();
    assert_eq!(download.status(), 200);
    assert_eq!(download.headers()["cache-control"], "no-store");
    assert_eq!(download.text().await.unwrap(), "hello");
    assert_eq!(client.get(&url).send().await.unwrap().status(), 403);
    assert_eq!(
        client
            .get(format!("http://{address}/api/files/download"))
            .send()
            .await
            .unwrap()
            .status(),
        400
    );
    check_download_symlink(socket, address, cwd).await;
    check_rebound_download(socket, address, cwd).await;
}

async fn check_download_symlink(socket: &mut Socket, address: &str, cwd: &std::path::Path) {
    use std::os::unix::fs::symlink;
    fs::write(cwd.join("different.txt"), "different").unwrap();
    symlink("file.txt", cwd.join("alias.txt")).unwrap();
    let token = request(
        socket,
        "fs.file.download_token.request",
        json!({"cwd":cwd,"path":"alias.txt"}),
    )
    .await;
    fs::remove_file(cwd.join("alias.txt")).unwrap();
    symlink("different.txt", cwd.join("alias.txt")).unwrap();
    let url = format!(
        "http://{address}/api/files/download?token={}",
        token["result"]["token"].as_str().unwrap()
    );
    let download = reqwest::get(url).await.unwrap();
    assert!(
        download.headers()["content-disposition"]
            .to_str()
            .unwrap()
            .contains("alias.txt")
    );
    assert_eq!(download.text().await.unwrap(), "hello");
}

async fn check_rebound_download(socket: &mut Socket, address: &str, cwd: &std::path::Path) {
    use std::os::unix::fs::symlink;
    let token = request(
        socket,
        "fs.file.download_token.request",
        json!({"cwd":cwd,"path":"file.txt"}),
    )
    .await;
    fs::rename(cwd.join("file.txt"), cwd.join("moved.txt")).unwrap();
    symlink("different.txt", cwd.join("file.txt")).unwrap();
    let url = format!(
        "http://{address}/api/files/download?token={}",
        token["result"]["token"].as_str().unwrap()
    );
    assert_eq!(reqwest::get(url).await.unwrap().status(), 404);
    fs::remove_file(cwd.join("file.txt")).unwrap();
    fs::rename(cwd.join("moved.txt"), cwd.join("file.txt")).unwrap();
}

#[tokio::test]
async fn file_errors_and_inline_content_preserve_paseo_shapes() {
    let temp = tempfile::tempdir().unwrap();
    let cwd = temp.path().join("workspace");
    fs::create_dir(&cwd).unwrap();
    fs::write(cwd.join("image.png"), [1, 2, 3]).unwrap();
    fs::write(cwd.join("binary"), [0, 1, 2]).unwrap();
    let log = temp.path().join("log");
    let mut process = start(&temp.path().join("state"), &log);
    let address = ready(&mut process, &log).await;
    let mut socket = connect(&address, server_filesystem::protocol::files::CAPABILITIES).await;
    let image = request(
        &mut socket,
        "fs.explorer.request",
        json!({"cwd":cwd,"path":"image.png","mode":"file"}),
    )
    .await;
    assert_eq!(image["result"]["file"]["encoding"], "base64");
    assert_eq!(image["result"]["file"]["content"], "AQID");
    let binary = request(
        &mut socket,
        "fs.explorer.request",
        json!({"cwd":cwd,"path":"binary","mode":"file"}),
    )
    .await;
    assert_eq!(binary["result"]["file"]["encoding"], "none");
    assert!(binary["result"]["file"].get("content").is_none());
    for (method, params) in [
        (
            "directory.suggestions.request",
            json!({"cwd":cwd,"query":"a","limit":0}),
        ),
        (
            "fs.explorer.request",
            json!({"cwd":cwd,"mode":"file","maxBytes":0}),
        ),
        (
            "fs.file.write.request",
            json!({"cwd":cwd,"path":"binary","content":"new"}),
        ),
    ] {
        assert_eq!(
            request(&mut socket, method, params).await["code"],
            "invalid_message"
        );
    }
    let missing = request(
        &mut socket,
        "fs.file.subscribe.request",
        json!({"cwd":cwd,"path":"absent","subscriptionId":"missing"}),
    )
    .await;
    assert_eq!(missing["result"]["initial"]["status"], "missing");
    let replacement = request(
        &mut socket,
        "fs.file.subscribe.request",
        json!({"cwd":cwd,"path":"binary","subscriptionId":"missing"}),
    )
    .await;
    assert_eq!(replacement["result"]["initial"]["status"], "ready");
    request(
        &mut socket,
        "fs.file.unsubscribe.request",
        json!({"subscriptionId":"missing"}),
    )
    .await;
    let outside = request(
        &mut socket,
        "fs.explorer.request",
        json!({"cwd":cwd,"path":"../log","mode":"file"}),
    )
    .await;
    assert_eq!(
        outside["result"]["error"],
        "Access outside of workspace is not allowed"
    );
    let token = request(
        &mut socket,
        "fs.file.download_token.request",
        json!({"cwd":cwd,"path":"binary"}),
    )
    .await;
    fs::remove_file(cwd.join("binary")).unwrap();
    let url = format!(
        "http://{address}/api/files/download?token={}",
        token["result"]["token"].as_str().unwrap()
    );
    assert_eq!(reqwest::get(url).await.unwrap().status(), 404);
    socket.close(None).await.unwrap();
    terminate(&mut process).await;
}

async fn check_subscription(
    socket: &mut Socket,
    address: &str,
    cwd: &std::path::Path,
    methods: &[&str],
) {
    let initial = request(
        socket,
        "fs.file.subscribe.request",
        json!({"cwd":cwd,"path":"file.txt","subscriptionId":"file"}),
    )
    .await;
    assert_eq!(initial["result"]["initial"]["status"], "ready");
    let mut other = connect(address, methods).await;
    request(
        &mut other,
        "fs.file.unsubscribe.request",
        json!({"subscriptionId":"file"}),
    )
    .await;
    fs::write(cwd.join("file.txt"), "external edit").unwrap();
    let update = receive(socket).await;
    assert_eq!(update["method"], "fs.file.update");
    assert_eq!(update["params"]["version"]["size"], 13);
    fs::remove_file(cwd.join("file.txt")).unwrap();
    assert_eq!(
        receive(socket).await["params"]["version"]["status"],
        "missing"
    );
    fs::write(cwd.join("file.txt"), "restored").unwrap();
    assert_eq!(
        receive(socket).await["params"]["version"]["status"],
        "ready"
    );
    request(
        socket,
        "subscription.release.request",
        json!({"subscriptionId":"file"}),
    )
    .await;
    fs::write(cwd.join("file.txt"), "released").unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(450), socket.next())
            .await
            .is_err()
    );
    other.close(None).await.unwrap();
}

async fn send_request(socket: &mut Socket, id: &str, method: &str, params: Value) {
    socket
        .send(Message::Text(
            json!({"type":"request","request_id":id,"method":method,"params":params})
                .to_string()
                .into(),
        ))
        .await
        .unwrap();
}
