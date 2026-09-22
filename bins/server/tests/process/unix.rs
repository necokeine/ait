use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const CREDENTIAL_SENTINEL: &str = "offline-credential-sentinel-must-never-be-persisted";

const TOKEN: &str = "offline-process-token-at-least-32-characters";

struct Process(Child);

impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn start(directory: &Path, log: &Path) -> Process {
    Process(
        Command::new(env!("CARGO_BIN_EXE_server"))
            .args([
                "--data-dir",
                directory.to_str().unwrap(),
                "--listen",
                "127.0.0.1:0",
                "--log-level",
                "info",
            ])
            .env("AIT_SERVER_TOKEN", TOKEN)
            .env("AIT_SERVER_CREDENTIAL_TEST", CREDENTIAL_SENTINEL)
            .env("HOME", directory.parent().unwrap())
            .env_remove("AIT_SERVER_LISTEN")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(std::fs::File::create(log).unwrap())
            .spawn()
            .unwrap(),
    )
}

#[path = "transport.rs"]
mod transport;

#[path = "agents.rs"]
mod agents;

#[path = "agent_runtime.rs"]
mod agent_runtime;

#[path = "projects.rs"]
mod projects;

#[path = "directory.rs"]
mod directory;

#[path = "daemon.rs"]
mod daemon;

#[path = "workspace_labels.rs"]
mod workspace_labels;

#[path = "worktrees.rs"]
mod worktrees;

#[path = "workspace_automation.rs"]
mod workspace_automation;

async fn ready(process: &mut Process, log: &Path) -> String {
    let start = Instant::now();
    loop {
        let text = std::fs::read_to_string(log).unwrap();
        if let Some(address) = text.lines().find_map(|line| {
            line.split_once("listen=")
                .map(|(_, address)| address.trim().to_owned())
        }) {
            assert!(!text.contains(TOKEN));
            return address;
        }
        assert!(process.0.try_wait().unwrap().is_none(), "{text}");
        assert!(
            start.elapsed() < Duration::from_secs(30),
            "startup timeout: {text}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

async fn terminate(process: &mut Process) {
    assert!(
        Command::new("kill")
            .args(["-TERM", &process.0.id().to_string()])
            .status()
            .unwrap()
            .success()
    );
    let start = Instant::now();
    loop {
        if let Some(status) = process.0.try_wait().unwrap() {
            assert!(status.success());
            break;
        }
        assert!(
            start.elapsed() < Duration::from_secs(20),
            "shutdown timeout"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn signal_shutdown_releases_process_lock_and_preserves_identity() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("server");
    let log = root.path().join("server.log");
    let mut first = start(&directory, &log);
    let address = ready(&mut first, &log).await;
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let info: serde_json::Value = client
        .get(format!("http://{address}/v1/server/info"))
        .bearer_auth(TOKEN)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let second = Command::new(env!("CARGO_BIN_EXE_server"))
        .args([
            "--data-dir",
            directory.to_str().unwrap(),
            "--listen",
            "127.0.0.1:0",
        ])
        .env("AIT_SERVER_TOKEN", TOKEN)
        .output()
        .unwrap();
    assert!(!second.status.success());
    assert!(String::from_utf8_lossy(&second.stderr).contains("already in use"));
    assert!(!String::from_utf8_lossy(&second.stderr).contains(TOKEN));
    terminate(&mut first).await;
    let mut restarted = start(&directory, &log);
    let address = ready(&mut restarted, &log).await;
    let next: serde_json::Value = client
        .get(format!("http://{address}/v1/server/info"))
        .bearer_auth(TOKEN)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(info["server_id"], next["server_id"]);
    assert_ne!(info["instance_id"], next["instance_id"]);
    terminate(&mut restarted).await;
}
