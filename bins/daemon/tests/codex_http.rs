//! End-to-end coverage for Codex response generation through the daemon HTTP API.

#![cfg(unix)]

use std::{
    env,
    fs::{self, File},
    net::{SocketAddr, TcpListener},
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use reqwest::Client;
use serde_json::{Value, json};
use tempfile::TempDir;

const ASSISTANT_RESPONSE: &str = "Generated through Codex over the daemon HTTP API.";

struct DaemonGuard {
    child: Child,
    log_path: PathBuf,
}

impl DaemonGuard {
    fn assert_running(&mut self) {
        if let Some(status) = self.child.try_wait().unwrap() {
            panic!(
                "ait-daemon exited with {status}: {}",
                fs::read_to_string(&self.log_path).unwrap_or_default()
            );
        }
    }
}

impl Drop for DaemonGuard {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[tokio::test]
#[allow(
    clippy::too_many_lines,
    reason = "the daemon acceptance flow intentionally remains one end-to-end scenario"
)]
async fn daemon_http_generates_an_assistant_response_through_codex() {
    let temporary = TempDir::new().unwrap();
    let project = temporary.path().join("project");
    fs::create_dir(&project).unwrap();
    assert!(
        Command::new("git")
            .arg("-C")
            .arg(&project)
            .args(["init", "--quiet"])
            .status()
            .unwrap()
            .success()
    );

    let codex_log = temporary.path().join("codex.jsonl");
    let fake_bin = temporary.path().join("bin");
    fs::create_dir(&fake_bin).unwrap();
    install_fake_codex(&fake_bin.join("codex"));

    let address = unused_loopback_address();
    let daemon_log = temporary.path().join("daemon.log");
    let log = File::create(&daemon_log).unwrap();
    let mut search_paths = vec![fake_bin];
    search_paths.extend(env::split_paths(&env::var_os("PATH").unwrap_or_default()));
    let child = Command::new(env!("CARGO_BIN_EXE_ait-daemon"))
        .args([
            "--database",
            temporary.path().join("ait.sqlite3").to_str().unwrap(),
            "--listen",
            &address.to_string(),
        ])
        .env("PATH", env::join_paths(search_paths).unwrap())
        .env("AIT_FAKE_CODEX_LOG", &codex_log)
        .stdout(Stdio::null())
        .stderr(Stdio::from(log))
        .spawn()
        .unwrap();
    let mut daemon = DaemonGuard {
        child,
        log_path: daemon_log,
    };

    let base_url = format!("http://{address}");
    let client = Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();
    wait_until_ready(&client, &base_url, &mut daemon).await;
    register_test_entities(&client, &base_url, &project).await;

    // Keep one SSE response completely unread. Its socket can back up while
    // the provider emits thousands of deltas, but Run persistence must remain
    // independent of subscriber consumption.
    let stalled_stream = client
        .get(format!("{base_url}/v1/event/stream?after=0"))
        .send()
        .await
        .unwrap();
    assert_eq!(stalled_stream.status(), reqwest::StatusCode::OK);

    let submitted_at = Instant::now();
    let response = post(
        &client,
        &base_url,
        "/v1/session/submit-message",
        &json!({
            "session_id": "daemon-codex-session",
            "text": "Generate a response through Codex.",
        }),
    )
    .await;
    assert_ok(&response);
    assert_eq!(response["result"]["kind"], "run");
    assert_eq!(response["result"]["value"]["status"], "queued");
    assert!(submitted_at.elapsed() < Duration::from_millis(500));
    let run_id = response["result"]["value"]["id"].as_str().unwrap();

    let progress_deadline = Instant::now() + Duration::from_secs(2);
    let progress = loop {
        let progress: Value = client
            .get(format!("{base_url}/v1/run/progress"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        if let Some(checkpoint) = progress.as_array().and_then(|values| values.first()) {
            break checkpoint.clone();
        }
        assert!(
            Instant::now() < progress_deadline,
            "progress was not visible"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    };
    assert_eq!(progress["run_id"], run_id);
    assert!(
        progress["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| { item["type"] == "message" && item["text"] == "Inspecting the project." })
    );
    assert!(progress["items"].as_array().unwrap().iter().any(|item| {
        item["type"] == "operation" && item["operation"]["status"] == "inProgress"
    }));

    let replay = client
        .get(format!("{base_url}/v1/event/list?after=0&limit=1000"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(replay.contains("run.progress"));
    assert!(replay.contains("text_delta"));

    let completion_deadline = Instant::now() + Duration::from_secs(4);
    loop {
        let run = post(
            &client,
            &base_url,
            "/v1/run/get",
            &json!({"run_id": run_id}),
        )
        .await;
        assert_ok(&run);
        if run["result"]["value"]["status"] == "completed" {
            break;
        }
        assert!(Instant::now() < completion_deadline, "Run did not complete");
        tokio::time::sleep(Duration::from_millis(25)).await;
    }

    let snapshot: Value = client
        .get(format!("{base_url}/v1/workspace/snapshot"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_ok(&snapshot);
    let messages = snapshot["result"]["value"]["messages"].as_array().unwrap();
    assert!(messages.iter().any(|message| {
        message["role"] == "assistant" && message["text"] == ASSISTANT_RESPONSE
    }));

    let protocol = fs::read_to_string(codex_log).unwrap();
    assert!(protocol.contains("\"method\":\"initialize\""));
    assert!(protocol.contains("\"method\":\"turn/start\""));
    assert!(protocol.contains("\"effort\":\"high\""));
    assert!(protocol.contains("Generate a response through Codex."));
    drop(stalled_stream);
}

fn unused_loopback_address() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener);
    address
}

async fn wait_until_ready(client: &Client, base_url: &str, daemon: &mut DaemonGuard) {
    for _ in 0..100 {
        if client
            .get(format!("{base_url}/v1/workspace/snapshot"))
            .send()
            .await
            .is_ok()
        {
            return;
        }
        daemon.assert_running();
        thread::sleep(Duration::from_millis(25));
    }
    panic!(
        "ait-daemon did not become ready: {}",
        fs::read_to_string(&daemon.log_path).unwrap_or_default()
    );
}

async fn post(client: &Client, base_url: &str, route: &str, body: &Value) -> Value {
    client
        .post(format!("{base_url}{route}"))
        .json(body)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap()
}

async fn register_test_entities(client: &Client, base_url: &str, project: &Path) {
    for (route, body) in [
        (
            "/v1/project/register",
            json!({
                "id": "daemon-codex-project",
                "name": "Daemon Codex Test",
                "workdir": project,
            }),
        ),
        (
            "/v1/agent/register",
            json!({
                "id": "daemon-codex-agent",
                "name": "Codex",
                "config": { "provider_id": "builtin-codex", "model": "gpt-5.6-sol", "reasoning_effort": "high" },
            }),
        ),
        (
            "/v1/session/create",
            json!({
                "id": "daemon-codex-session",
                "project_id": "daemon-codex-project",
                "agent_id": "daemon-codex-agent",
            }),
        ),
    ] {
        assert_ok(&post(client, base_url, route, &body).await);
    }
}

fn assert_ok(response: &Value) {
    assert_eq!(response["ok"], true, "daemon response: {response}");
}

fn install_fake_codex(path: &Path) {
    fs::write(
        path,
        format!(
            r#"#!/bin/sh
[ "$1" = "app-server" ] || exit 2
read_line() {{
  IFS= read -r line || exit 3
  printf '%s\n' "$line" >> "$AIT_FAKE_CODEX_LOG"
}}
read_line
printf '%s\n' '{{"id":0,"result":{{}}}}'
read_line
read_line
case "$line" in
  *'"method":"config/read"'*) ;;
  *) exit 4 ;;
esac
printf '%s\n' '{{"id":-1,"result":{{"config":{{"shell_environment_policy":{{}}}},"origins":{{}}}}}}'
read_line
case "$line" in
  *'"model":"gpt-5.6-sol"'*) ;;
  *) printf '%s\n' '{{"id":1,"error":{{"code":-32602,"message":"unsupported model"}}}}'; exit 5 ;;
esac
printf '%s\n' '{{"id":1,"result":{{"thread":{{"id":"thread-http-test"}}}}}}'
read_line
printf '%s\n' '{{"id":2,"result":{{"turn":{{"id":"turn-http-test"}}}}}}'
printf '%s\n' '{{"method":"item/started","params":{{"threadId":"thread-http-test","turnId":"turn-http-test","item":{{"type":"agentMessage","id":"commentary-http-test","phase":"commentary","text":""}}}}}}'
printf '%s\n' '{{"method":"item/agentMessage/delta","params":{{"threadId":"thread-http-test","turnId":"turn-http-test","itemId":"commentary-http-test","delta":"Inspecting the project."}}}}'
printf '%s\n' '{{"method":"item/completed","params":{{"threadId":"thread-http-test","turnId":"turn-http-test","item":{{"type":"agentMessage","id":"commentary-http-test","phase":"commentary","text":"Inspecting the project."}}}}}}'
printf '%s\n' '{{"method":"item/started","params":{{"threadId":"thread-http-test","turnId":"turn-http-test","item":{{"type":"commandExecution","id":"command-http-test","status":"inProgress","command":"pwd"}}}}}}'
printf '%s\n' '{{"method":"item/started","params":{{"threadId":"thread-http-test","turnId":"turn-http-test","item":{{"type":"agentMessage","id":"stress-http-test","phase":"commentary","text":""}}}}}}'
i=0
while [ "$i" -lt 4000 ]; do
  printf '%s\n' '{{"method":"item/agentMessage/delta","params":{{"threadId":"thread-http-test","turnId":"turn-http-test","itemId":"stress-http-test","delta":"x"}}}}'
  i=$((i + 1))
done
printf '%s\n' '{{"method":"item/completed","params":{{"threadId":"thread-http-test","turnId":"turn-http-test","item":{{"type":"agentMessage","id":"stress-http-test","phase":"commentary","text":"Stress replay complete."}}}}}}'
sleep 0.6
printf '%s\n' '{{"method":"item/completed","params":{{"threadId":"thread-http-test","turnId":"turn-http-test","item":{{"type":"commandExecution","id":"command-http-test","status":"completed","command":"pwd","aggregatedOutput":"project"}}}}}}'
printf '%s\n' '{{"method":"item/started","params":{{"threadId":"thread-http-test","turnId":"turn-http-test","item":{{"type":"agentMessage","id":"assistant-http-test","phase":"final_answer","text":""}}}}}}'
printf '%s\n' '{{"method":"item/agentMessage/delta","params":{{"threadId":"thread-http-test","turnId":"turn-http-test","itemId":"assistant-http-test","delta":"Generated through Codex "}}}}'
sleep 0.6
printf '%s\n' '{{"method":"item/completed","params":{{"threadId":"thread-http-test","turnId":"turn-http-test","item":{{"type":"agentMessage","id":"assistant-http-test","phase":"final_answer","text":"{ASSISTANT_RESPONSE}"}}}}}}'
printf '%s\n' '{{"method":"turn/completed","params":{{"threadId":"thread-http-test","turn":{{"id":"turn-http-test","items":[],"status":"completed"}}}}}}'
"#
        ),
    )
    .unwrap();
    let mut permissions = fs::metadata(path).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).unwrap();
}
