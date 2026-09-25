use std::path::Path;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::transport::{connect, request};
use super::{Process, TOKEN, ready, start, start_with_path, terminate};

const READ_CAPABILITIES: &[&str] = &[
    "daemon.get_status.request",
    "daemon.get_pairing_offer.request",
    "daemon.config.reload.request",
    "daemon.update.request",
    "diagnostics.request",
    "daemon.config.get.request",
    "daemon.config.set.request",
];

#[tokio::test]
async fn missing_provider_is_reported_and_editors_return_migration_responses() {
    let root = tempfile::tempdir().unwrap();
    let log = root.path().join("server.log");
    let mut process = start_with_path(
        &root.path().join("state"),
        &log,
        Some(root.path().as_os_str()),
    );
    let address = ready(&mut process, &log).await;
    let mut socket = connect(
        &address,
        &[
            "daemon.get_status.request",
            "diagnostics.request",
            "editor.available.list.request",
            "editor.open.request",
        ],
    )
    .await;
    let status = request(&mut socket, "daemon.get_status.request", json!({})).await;
    assert_eq!(status["result"]["providers"][0]["provider"], "codex");
    assert_eq!(status["result"]["providers"][0]["available"], false);
    let diagnostic = request(&mut socket, "diagnostics.request", json!({})).await;
    let report = diagnostic["result"]["diagnostic"].as_str().unwrap();
    assert!(report.contains("Total: 1"));
    assert!(report.contains("Available: 0"));
    assert!(report.contains("codex: unavailable"));
    let editors = request(&mut socket, "editor.available.list.request", json!({})).await;
    assert_eq!(editors["result"]["editors"], json!([]));
    let opened = request(
        &mut socket,
        "editor.open.request",
        json!({"path":"/missing","editorId":"code"}),
    )
    .await;
    assert_eq!(opened["result"]["error"], editors["result"]["error"]);
    assert!(
        opened["result"]["error"]
            .as_str()
            .unwrap()
            .contains("desktop app")
    );
    drop(socket);
    terminate(&mut process).await;
}

async fn info(address: &str) -> Value {
    reqwest::Client::builder()
        .no_proxy()
        .build()
        .unwrap()
        .get(format!("http://{address}/v1/server/info"))
        .bearer_auth(TOKEN)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap()
}

async fn wait_for_restart(process: &mut Process, log: &Path) -> String {
    let started = Instant::now();
    loop {
        let text = std::fs::read_to_string(log).unwrap();
        let addresses: Vec<_> = text
            .lines()
            .filter_map(|line| line.split_once("listen=").map(|(_, value)| value.trim()))
            .collect();
        if addresses.len() >= 2 {
            return addresses.last().unwrap().to_string();
        }
        assert!(process.0.try_wait().unwrap().is_none(), "{text}");
        assert!(started.elapsed() < Duration::from_secs(30), "{text}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

async fn wait_for_exit(process: &mut Process) {
    let started = Instant::now();
    loop {
        if let Some(status) = process.0.try_wait().unwrap() {
            assert!(status.success());
            return;
        }
        assert!(started.elapsed() < Duration::from_secs(20));
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn daemon_status_config_diagnostics_and_update_match_canonical_contract() {
    let fixture = super::native::NativeFixture::new();
    let root = &fixture.root;
    let directory = root.path().join("server");
    let log = root.path().join("server.log");
    let mut process = start_with_path(&directory, &log, Some(&fixture.path));
    let address = ready(&mut process, &log).await;
    let mut socket = connect(&address, READ_CAPABILITIES).await;

    let status = request(&mut socket, "daemon.get_status.request", json!({})).await;
    assert_eq!(status["type"], "response");
    assert_eq!(status["result"]["listen"], address);
    assert!(status["result"]["serverId"].as_str().is_some());
    assert_eq!(status["result"]["pid"], process.0.id());
    assert_eq!(status["result"]["relay"], Value::Null);
    assert_eq!(
        status["result"]["providers"],
        json!([{ "provider":"codex", "available":true, "error":null }])
    );

    let pairing = request(&mut socket, "daemon.get_pairing_offer.request", json!({})).await;
    assert_eq!(pairing["code"], "unsupported_capability");
    let config = request(&mut socket, "daemon.config.get.request", json!({})).await;
    assert_eq!(config["result"]["config"]["relay"]["enabled"], false);
    assert_eq!(config["result"]["config"]["mcp"]["injectIntoAgents"], false);

    let changed = request(
        &mut socket,
        "daemon.config.set.request",
        json!({"config":{
            "browserTools":{"enabled":true},
            "providers":{"codex":{"enabled":false}},
            "futureIgnored":true
        }}),
    )
    .await;
    assert_eq!(changed["result"]["config"]["browserTools"]["enabled"], true);
    assert!(changed["result"]["config"].get("futureIgnored").is_none());
    let persisted = std::fs::read_to_string(directory.join("config.json")).unwrap();
    assert!(!persisted.contains(TOKEN));
    let mut external: Value = serde_json::from_str(&persisted).unwrap();
    external["autoArchiveAfterMerge"] = json!(true);
    std::fs::write(
        directory.join("config.json"),
        serde_json::to_vec_pretty(&external).unwrap(),
    )
    .unwrap();
    let reload = request(&mut socket, "daemon.config.reload.request", json!({})).await;
    assert_eq!(
        reload["result"]["appliedPaths"],
        json!(["daemon.autoArchiveAfterMerge"])
    );
    assert_eq!(
        request(&mut socket, "daemon.config.get.request", json!({})).await["result"]["config"]["autoArchiveAfterMerge"],
        true
    );

    let diagnostic = request(&mut socket, "diagnostics.request", json!({})).await;
    let diagnostic = diagnostic["result"]["diagnostic"].as_str().unwrap();
    assert!(diagnostic.contains("Paseo diagnostics"));
    assert!(diagnostic.contains("daemon.get_status.request"));
    assert!(!diagnostic.contains(TOKEN));
    assert!(diagnostic.contains("Total: 1"));
    assert!(diagnostic.contains("codex: available"));

    let update = request(&mut socket, "daemon.update.request", json!({})).await;
    assert_eq!(update["result"]["success"], false);
    assert_eq!(update["result"]["newVersion"], Value::Null);
    assert!(
        update["result"]["error"]
            .as_str()
            .unwrap()
            .contains("standalone Rust server")
    );

    let legacy = request(&mut socket, "get_daemon_config_request", json!({})).await;
    assert_eq!(legacy["code"], "method_not_found");
    terminate(&mut process).await;
}

#[tokio::test]
async fn websocket_restart_rebuilds_server_and_shutdown_exits_process() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("server");
    let log = root.path().join("server.log");
    let mut process = start(&directory, &log);
    let first_address = ready(&mut process, &log).await;
    let first_info = info(&first_address).await;
    let mut socket = connect(&first_address, &["server.restart.request"]).await;
    let response = request(
        &mut socket,
        "server.restart.request",
        json!({"reason":"settings_changed"}),
    )
    .await;
    assert_eq!(
        response["result"],
        json!({"status":"restart_requested","reason":"settings_changed"})
    );

    let second_address = wait_for_restart(&mut process, &log).await;
    let second_info = info(&second_address).await;
    assert_eq!(first_info["server_id"], second_info["server_id"]);
    assert_ne!(first_info["instance_id"], second_info["instance_id"]);

    let mut socket = connect(&second_address, &["server.shutdown.request"]).await;
    let response = request(&mut socket, "server.shutdown.request", json!({})).await;
    assert_eq!(response["result"], json!({"status":"shutdown_requested"}));
    wait_for_exit(&mut process).await;
}
