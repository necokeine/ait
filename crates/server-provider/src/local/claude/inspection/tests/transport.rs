use super::*;
use tokio::io::AsyncWriteExt;

async fn serve(status: &str, body: &str) -> (String, tokio::task::JoinHandle<String>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}/usage", listener.local_addr().unwrap());
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let task = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        while !request.ends_with(b"\r\n\r\n") {
            request.push(socket.read_u8().await.unwrap());
            assert!(request.len() < 8192);
        }
        let _ = socket.write_all(response.as_bytes()).await;
        String::from_utf8(request).unwrap()
    });
    (endpoint, task)
}

#[tokio::test]
async fn quota_http_is_read_only_bounded_and_refuses_auth_errors_or_redirects() {
    let token = SecretString::from("offline-test-token");
    for (status, body, success) in [
        (
            "200 OK",
            "{\"five_hour\":{\"utilization\":0}}".to_owned(),
            true,
        ),
        ("401 Unauthorized", "{}".to_owned(), false),
        ("403 Forbidden", "{}".to_owned(), false),
        ("302 Found", "{}".to_owned(), false),
        ("429 Too Many Requests", "{}".to_owned(), false),
        ("200 OK", "invalid".to_owned(), false),
        ("200 OK", "x".repeat(256 * 1024 + 1), false),
    ] {
        let (endpoint, task) = serve(status, &body).await;
        let result = fetch(&endpoint, &token, Duration::from_secs(5)).await;
        assert_eq!(result.is_ok(), success, "{status}");
        let request = task.await.unwrap().to_ascii_lowercase();
        assert!(request.starts_with("get /usage http/1.1"));
        assert!(request.contains("authorization: bearer offline-test-token"));
        assert!(request.contains("anthropic-beta: oauth-2025-04-20"));
    }
}

#[cfg(unix)]
#[tokio::test]
async fn native_auth_output_limits_timeout_and_preserves_logged_out_json() {
    for (script, success) in [
        ("printf '{\"loggedIn\":false}'; exit 1", true),
        ("exit 1", false),
        ("head -c 65537 /dev/zero", false),
    ] {
        let mut command = tokio::process::Command::new("/bin/sh");
        command.args(["-c", script]);
        assert_eq!(
            output(command, Duration::from_secs(5)).await.is_ok(),
            success
        );
    }
    let mut command = tokio::process::Command::new("/bin/sleep");
    command.arg("5");
    assert!(output(command, Duration::from_millis(50)).await.is_err());
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source");
    let link = root.path().join("linked");
    std::fs::write(&source, "x".repeat(65537)).unwrap();
    assert!(read_file(&source).is_err());
    std::os::unix::fs::symlink(&source, &link).unwrap();
    assert!(read_file(&link).is_err());
}
