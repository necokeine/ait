use super::*;
use crate::config::Cli;
use clap::Parser;

fn config(directory: &std::path::Path) -> Config {
    Config::load(
        Cli::parse_from([
            "server",
            "--data-dir",
            directory.to_str().unwrap(),
            "--listen",
            "127.0.0.1:0",
        ]),
        |name| {
            (name == "AIT_SERVER_TOKEN")
                .then(|| "offline-host-test-token-at-least-32-characters".into())
        },
    )
    .unwrap()
}

#[tokio::test]
async fn binding_failure_has_no_state_side_effects() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("absent");
    let reserved = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut configuration = config(&directory);
    configuration.listen = reserved.local_addr().unwrap();
    assert!(Server::bind(configuration).await.is_err());
    assert!(!directory.exists());
}

#[tokio::test]
async fn shutdown_releases_lock_and_restart_keeps_only_stable_identity() {
    let root = tempfile::tempdir().unwrap();
    let server = Server::bind(config(root.path())).await.unwrap();
    let address = server.address();
    assert!(Server::bind(config(root.path())).await.is_err());
    let (stop, signal) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(server.serve(async move {
        let _ = signal.await;
    }));
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let first: serde_json::Value = client
        .get(format!("http://{address}/v1/server/info"))
        .bearer_auth("offline-host-test-token-at-least-32-characters")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    stop.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let restarted = Server::bind(config(root.path())).await.unwrap();
    assert_eq!(restarted.instance.server_id.to_string(), first["server_id"]);
    assert_ne!(
        restarted.instance.instance_id.to_string(),
        first["instance_id"]
    );
    restarted.serve(async {}).await.unwrap();
}
