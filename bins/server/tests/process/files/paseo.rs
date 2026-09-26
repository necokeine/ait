//! Paseo owned-subscriptions/file-upload cases over the production server transport.

use super::*;

struct Fixture {
    process: super::super::Process,
    address: String,
    root: tempfile::TempDir,
}

impl Fixture {
    async fn start() -> Self {
        let root = tempfile::tempdir().unwrap();
        let log = root.path().join("server.log");
        let mut process = start(&root.path().join("state"), &log);
        let address = ready(&mut process, &log).await;
        Self {
            process,
            address,
            root,
        }
    }

    async fn socket(&self) -> Socket {
        let mut methods = server_filesystem::protocol::files::CAPABILITIES.to_vec();
        methods.extend(["subscription.release.request", "connection.ping"]);
        connect(&self.address, &methods).await
    }

    async fn stop(mut self) {
        terminate(&mut self.process).await;
    }
}

async fn fence(socket: &mut Socket) {
    let response = request(socket, "connection.ping", json!({"nonce":"source-fence"})).await;
    assert_eq!(
        response["result"]["nonce"], "source-fence",
        "unexpected delivery: {response}"
    );
}

async fn start_upload(socket: &mut Socket, name: &str) {
    send_request(socket, "same-upload", "file.upload.request", json!({"fileName":name,"mimeType":"text/plain","size":5,"modifiedAt":"2026-09-26T00:00:00Z"})).await;
    fence(socket).await;
    send_frame(socket, "same-upload", begin(5)).await;
    fence(socket).await;
}

async fn chunk(socket: &mut Socket, bytes: &[u8]) {
    send_frame(socket, "same-upload", FileFrame::Chunk(bytes.to_vec())).await;
    // Rust shares one blocking catalog/file admission slot across connections.
    // Fence each accepted frame so this test isolates ownership from backpressure rejection.
    fence(socket).await;
}

fn uploaded(response: &Value, name: &str, bytes: &[u8]) -> std::path::PathBuf {
    assert_eq!(response["request_id"], "same-upload");
    assert_eq!(response["type"], "response");
    assert!(response["result"]["error"].is_null(), "{response}");
    let file = &response["result"]["file"];
    assert_eq!(file["fileName"], name);
    let path = std::path::PathBuf::from(file["path"].as_str().unwrap());
    assert_eq!(fs::read(&path).unwrap(), bytes);
    path
}

#[tokio::test]
async fn identical_upload_ids_in_a_shared_logical_session_keep_independent_files_and_replies() {
    let fixture = Fixture::start().await;
    let mut first = fixture.socket().await;
    let mut second = fixture.socket().await;
    let mut idle = fixture.socket().await;
    start_upload(&mut first, "a.txt").await;
    start_upload(&mut second, "b.txt").await;
    chunk(&mut first, b"al").await;
    chunk(&mut second, b"bra").await;
    chunk(&mut first, b"pha").await;
    chunk(&mut second, b"vo").await;
    send_frame(&mut second, "same-upload", FileFrame::End).await;
    let second_result = receive(&mut second).await;
    send_frame(&mut first, "same-upload", FileFrame::End).await;
    let first_result = receive(&mut first).await;
    let first_path = uploaded(&first_result, "a.txt", b"alpha");
    let second_path = uploaded(&second_result, "b.txt", b"bravo");
    assert_ne!(first_path, second_path);
    assert_ne!(
        first_result["result"]["file"]["id"],
        second_result["result"]["file"]["id"]
    );
    fence(&mut first).await;
    fence(&mut second).await;
    fence(&mut idle).await;
    fixture.stop().await;
}

#[tokio::test]
async fn disconnecting_one_upload_owner_cleans_only_its_partial_file() {
    let fixture = Fixture::start().await;
    let mut dropped = fixture.socket().await;
    let mut retained = fixture.socket().await;
    start_upload(&mut dropped, "dropped.txt").await;
    start_upload(&mut retained, "retained.txt").await;
    chunk(&mut dropped, b"part").await;
    chunk(&mut retained, b"ke").await;
    fence(&mut dropped).await;
    fence(&mut retained).await;
    dropped.close(None).await.unwrap();
    drop(dropped);
    chunk(&mut retained, b"pt!").await;
    send_frame(&mut retained, "same-upload", FileFrame::End).await;
    let result = receive(&mut retained).await;
    let completed = uploaded(&result, "retained.txt", b"kept!");
    let retained_directory = completed.parent().unwrap().canonicalize().unwrap();
    let mut remaining_directories = Vec::new();
    let cleanup = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            remaining_directories = fs::read_dir(fixture.root.path().join("state/uploads"))
                .unwrap()
                .map(|entry| entry.unwrap().path().canonicalize().unwrap())
                .collect::<Vec<_>>();
            if remaining_directories == std::slice::from_ref(&retained_directory) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    assert!(
        cleanup.is_ok(),
        "Remaining upload directories: {remaining_directories:?}"
    );
    fence(&mut retained).await;
    fixture.stop().await;
}

#[tokio::test]
async fn replacing_an_upload_id_removes_old_bytes_and_uses_the_latest_request_metadata() {
    let fixture = Fixture::start().await;
    let mut socket = fixture.socket().await;
    start_upload(&mut socket, "old.txt").await;
    send_frame(
        &mut socket,
        "same-upload",
        FileFrame::Chunk(b"old".to_vec()),
    )
    .await;
    fence(&mut socket).await;
    start_upload(&mut socket, "new.txt").await;
    send_frame(
        &mut socket,
        "same-upload",
        FileFrame::Chunk(b"fresh".to_vec()),
    )
    .await;
    send_frame(&mut socket, "same-upload", FileFrame::End).await;
    let result = receive(&mut socket).await;
    uploaded(&result, "new.txt", b"fresh");
    assert_eq!(
        fs::read_dir(fixture.root.path().join("state/uploads"))
            .unwrap()
            .count(),
        1
    );
    fence(&mut socket).await;
    fixture.stop().await;
}

#[tokio::test]
async fn identical_file_observations_keep_independent_release_and_idle_peers_receive_no_updates() {
    let fixture = Fixture::start().await;
    let mut owner = fixture.socket().await;
    let mut idle = fixture.socket().await;
    let cwd = fixture.root.path().join("workspace");
    fs::create_dir(&cwd).unwrap();
    fs::write(cwd.join("file.txt"), "initial").unwrap();
    for id in ["first", "second"] {
        let result = request(
            &mut owner,
            "fs.file.subscribe.request",
            json!({"cwd":cwd,"path":"file.txt","subscriptionId":id}),
        )
        .await;
        assert_eq!(result["result"]["subscriptionId"], id);
        assert_eq!(result["result"]["initial"]["status"], "ready");
    }
    fs::write(cwd.join("file.txt"), "updated").unwrap();
    let both = [receive(&mut owner).await, receive(&mut owner).await];
    for id in ["first", "second"] {
        assert!(
            both.iter().any(|event| event["method"] == "fs.file.update"
                && event["params"]["subscriptionId"] == id)
        );
    }
    request(
        &mut owner,
        "subscription.release.request",
        json!({"subscriptionId":"second"}),
    )
    .await;
    fs::write(cwd.join("file.txt"), "surviving observer").unwrap();
    let remaining = receive(&mut owner).await;
    assert_eq!(remaining["params"]["subscriptionId"], "first");
    assert_eq!(remaining["params"]["version"]["size"], 18);
    fence(&mut owner).await;
    fence(&mut idle).await;
    fixture.stop().await;
}
