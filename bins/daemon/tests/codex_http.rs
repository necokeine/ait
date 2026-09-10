//! End-to-end coverage for Codex response generation through the daemon HTTP API.

#![cfg(unix)]

use std::{
    env,
    fs::{self, File},
    net::{SocketAddr, TcpListener},
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::Arc,
    thread,
    time::{Duration, Instant},
};

use ait_application::LocalControlService;
use ait_contracts::{Command as ControlCommand, CommandResult};
use ait_domain::DomainError;
use ait_ports::{
    ControlChange, ControlFilter, ControlRecordKind, ControlStore, WorkspaceAgent,
    WorkspaceAgentInvocation, WorkspaceAgentResponse,
};
use ait_storage_sqlite::SqliteControlStore;
use async_trait::async_trait;
use reqwest::Client;
use serde_json::{Value, json};
use tempfile::TempDir;

const ASSISTANT_RESPONSE: &str = "Generated through Codex over the daemon HTTP API.";

struct DaemonGuard {
    child: Child,
    log_path: PathBuf,
}

struct SeedAgent;

#[async_trait]
impl WorkspaceAgent for SeedAgent {
    async fn invoke(
        &self,
        _request: WorkspaceAgentInvocation,
    ) -> Result<WorkspaceAgentResponse, DomainError> {
        Ok(WorkspaceAgentResponse {
            assistant_text: "seeded queued result".into(),
            commit_id: None,
            operations: Vec::new(),
            output_items: Vec::new(),
        })
    }
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
            .get(format!(
                "{base_url}/v1/run/progress?project_id=daemon-codex-project"
            ))
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

    let messages: Value = client
        .get(format!(
            "{base_url}/v1/message/list?project_id=daemon-codex-project"
        ))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_ok(&messages);
    let messages = messages["result"]["value"].as_array().unwrap();
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

#[tokio::test]
async fn daemon_is_ready_before_blocked_startup_recovery_and_executes_the_run_once() {
    const DESKTOP_READINESS_WINDOW: Duration = Duration::from_secs(15);
    let temporary = TempDir::new().unwrap();
    let project = temporary.path().join("project");
    fs::create_dir(&project).unwrap();
    let database = temporary.path().join("ait.sqlite3");
    let run_id = seed_queued_run(&database, &project).await;

    let codex_log = temporary.path().join("codex.jsonl");
    let fake_bin = temporary.path().join("bin");
    fs::create_dir(&fake_bin).unwrap();
    install_fake_codex(&fake_bin.join("codex"));
    let address = unused_loopback_address();
    let daemon_log = temporary.path().join("daemon.log");
    let log = File::create(&daemon_log).unwrap();
    let mut search_paths = vec![fake_bin];
    search_paths.extend(env::split_paths(&env::var_os("PATH").unwrap_or_default()));
    let started_at = Instant::now();
    let child = Command::new(env!("CARGO_BIN_EXE_ait-daemon"))
        .args([
            "--database",
            database.to_str().unwrap(),
            "--listen",
            &address.to_string(),
        ])
        .env("PATH", env::join_paths(search_paths).unwrap())
        .env("AIT_FAKE_CODEX_LOG", &codex_log)
        .env("AIT_FAKE_CODEX_DELAY_SECONDS", "16")
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
        .timeout(Duration::from_secs(2))
        .build()
        .unwrap();
    wait_until_ready(&client, &base_url, &mut daemon).await;
    assert!(started_at.elapsed() < DESKTOP_READINESS_WINDOW);

    tokio::time::sleep(
        DESKTOP_READINESS_WINDOW.saturating_sub(started_at.elapsed()) + Duration::from_millis(250),
    )
    .await;
    daemon.assert_running();
    let mut last_run = Value::Null;
    let completed = tokio::time::timeout(Duration::from_secs(6), async {
        loop {
            let runs: Value = client
                .get(format!(
                    "{base_url}/v1/run/list?project_id=recovery-project"
                ))
                .send()
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
            let run = runs["result"]["value"]
                .as_array()
                .unwrap()
                .iter()
                .find(|run| run["id"] == run_id)
                .unwrap();
            last_run = run.clone();
            if run["status"] == "completed" {
                break runs;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap_or_else(|error| {
        panic!(
            "recovered Run should complete after the blocked Agent is released: {error}; \
             last Run: {last_run}; daemon log: {}; Codex requests: {}",
            fs::read_to_string(&daemon.log_path).unwrap_or_default(),
            fs::read_to_string(&codex_log).unwrap_or_default(),
        )
    });
    assert_ok(&completed);
    daemon.assert_running();
    let protocol = fs::read_to_string(codex_log).unwrap();
    assert_eq!(protocol.matches("\"method\":\"turn/start\"").count(), 1);
}

#[tokio::test]
async fn bind_failure_does_not_claim_or_fence_a_queued_recovery() {
    let temporary = TempDir::new().unwrap();
    let project = temporary.path().join("project");
    fs::create_dir(&project).unwrap();
    let database = temporary.path().join("ait.sqlite3");
    let run_id = seed_queued_run(&database, &project).await;
    let store = SqliteControlStore::open(&database).unwrap();
    let filters = [
        ControlFilter::project(ControlRecordKind::Run, "recovery-project"),
        ControlFilter::project(ControlRecordKind::Session, "recovery-project"),
    ];
    let before = store.read(&filters).await.unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_ait-daemon"))
        .args([
            "--database",
            database.to_str().unwrap(),
            "--listen",
            &address.to_string(),
        ])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("Address already in use"));
    let after = store.read(&filters).await.unwrap();
    assert_eq!(after, before);
    let run = after
        .records
        .iter()
        .find(|record| record.kind == ControlRecordKind::Run && record.id == run_id)
        .unwrap();
    assert_eq!(run.value["status"], "queued");
    assert_eq!(run.value["phase"], "queued");
}

async fn seed_queued_run(database: &Path, project: &Path) -> String {
    let store = Arc::new(SqliteControlStore::open(database).unwrap());
    let service = LocalControlService::with_workspace_agent(store.clone(), Arc::new(SeedAgent));
    for command in [
        ControlCommand::RegisterProject {
            id: "recovery-project".into(),
            name: "Recovery Project".into(),
            workdir: project.display().to_string(),
            repo_url: None,
        },
        ControlCommand::RegisterAgent {
            id: "recovery-agent".into(),
            name: "Codex".into(),
            config: ait_contracts::AgentConfiguration {
                provider_id: "builtin-codex".into(),
                model: "gpt-5.6-sol".into(),
                reasoning_effort: Some("high".into()),
            },
        },
        ControlCommand::CreateSession {
            id: "recovery-session".into(),
            project_id: "recovery-project".into(),
            agent_id: "recovery-agent".into(),
            at_message_id: None,
        },
    ] {
        let response = service.execute(command).await;
        assert!(response.ok, "{:?}", response.error);
    }
    let response = service
        .execute(ControlCommand::SendMessage {
            session_id: "recovery-session".into(),
            text: "Recover this queued Run.".into(),
        })
        .await;
    let CommandResult::Run(completed) = response.result.unwrap() else {
        panic!("expected seeded Run");
    };
    assert_eq!(completed.status, "completed");

    let state = store
        .read(&[
            ControlFilter::id(ControlRecordKind::Run, &completed.id),
            ControlFilter::id(ControlRecordKind::Session, "recovery-session"),
        ])
        .await
        .unwrap();
    let mut run = state
        .records
        .iter()
        .find(|record| record.kind == ControlRecordKind::Run)
        .unwrap()
        .clone();
    run.value["status"] = Value::String("queued".into());
    run.value["phase"] = Value::String("queued".into());
    run.value["last_message_id"] = Value::Null;
    run.value["error"] = Value::Null;
    let mut session = state
        .records
        .iter()
        .find(|record| record.kind == ControlRecordKind::Session)
        .unwrap()
        .clone();
    session.value["active_run_id"] = Value::String(completed.id.clone());
    session.value["current_message_id"] = Value::String(completed.base_message_id.clone());
    store
        .apply(
            state.revision,
            vec![ControlChange::Put(run), ControlChange::Put(session)],
            Vec::new(),
        )
        .await
        .unwrap();
    completed.id
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
            .get(format!("{base_url}/v1/project/list"))
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
  *'"model":"gpt-5.6-sol"'*) ;;
  *) printf '%s\n' '{{"id":1,"error":{{"code":-32602,"message":"unsupported model"}}}}'; exit 4 ;;
esac
printf '%s\n' '{{"id":1,"result":{{"thread":{{"id":"thread-http-test"}}}}}}'
read_line
printf '%s\n' '{{"id":2,"result":{{"turn":{{"id":"turn-http-test"}}}}}}'
[ -z "$AIT_FAKE_CODEX_DELAY_SECONDS" ] || sleep "$AIT_FAKE_CODEX_DELAY_SECONDS"
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
