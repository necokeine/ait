//! Real CLI processes talking to an isolated production HTTP router and `SQLite` store.

use std::{path::PathBuf, process::Output, sync::Arc, time::Duration};

use ait_application::LocalControlService;
use ait_storage_sqlite::SqliteControlStore;
use serde_json::{Value, json};
use tempfile::TempDir;
use tokio::{net::TcpListener, process::Command, sync::oneshot, task::JoinHandle, time::timeout};

pub struct Workspace {
    pub directory: TempDir,
    pub endpoint: String,
    shutdown: Option<oneshot::Sender<()>>,
    server: Option<JoinHandle<()>>,
}

impl Workspace {
    pub async fn new() -> Self {
        let mut workspace = Self {
            directory: TempDir::new().unwrap(),
            endpoint: String::new(),
            shutdown: None,
            server: None,
        };
        workspace.start().await;
        workspace
    }

    async fn start(&mut self) {
        let store = SqliteControlStore::open(self.directory.path().join("ait.sqlite3")).unwrap();
        let service = Arc::new(LocalControlService::new(Arc::new(store)));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        self.endpoint = format!("http://{}", listener.local_addr().unwrap());
        let (shutdown, stopped) = oneshot::channel();
        self.shutdown = Some(shutdown);
        self.server = Some(tokio::spawn(async move {
            axum::serve(listener, ait_api_http::router(service))
                .with_graceful_shutdown(async {
                    let _ = stopped.await;
                })
                .await
                .unwrap();
        }));
    }

    pub async fn stop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            shutdown.send(()).unwrap();
        }
        if let Some(server) = &mut self.server {
            timeout(Duration::from_secs(10), server)
                .await
                .unwrap()
                .unwrap();
        }
        self.server = None;
    }

    pub async fn restart(&mut self) {
        self.stop().await;
        self.start().await;
    }

    pub fn path(&self, name: &str) -> PathBuf {
        self.directory.path().join(name)
    }

    pub async fn cli(&self, arguments: &[&str]) -> Output {
        // Never inherit a developer's proxy settings for the loopback test server.
        let mut process = Command::new(env!("CARGO_BIN_EXE_ait-cli"));
        process
            .args(["--endpoint", &self.endpoint])
            .args(arguments)
            .current_dir(self.directory.path())
            .env("NO_PROXY", "*")
            .env("no_proxy", "*")
            .kill_on_drop(true);
        timeout(Duration::from_secs(20), process.output())
            .await
            .expect("CLI exceeded its 20 second deadline")
            .expect("CLI could not start")
    }

    pub async fn command(&self, command: Value) -> Value {
        success(&self.cli(&["command", &command.to_string()]).await)
    }

    pub async fn reject(&self, command: Value, code: &str) {
        failure(&self.cli(&["command", &command.to_string()]).await, code);
    }

    pub async fn snapshot(&self) -> Value {
        success(&self.cli(&["snapshot"]).await)
    }

    pub async fn agent(&self, id: &str, mode: &str) -> Value {
        self.command(json!({
            "type": "register_agent", "id": id, "name": id,
            "model": "deterministic-v1", "mode": mode,
        }))
        .await
    }

    pub async fn project(&self, id: &str) -> Value {
        let path = self.path(id);
        std::fs::create_dir(&path).unwrap();
        self.command(json!({
            "type": "register_project", "id": id, "name": "工作项目",
            "workdir": path,
        }))
        .await
    }

    pub async fn session(&self, id: &str, project: &str, agent: &str) -> Value {
        self.command(json!({
            "type": "create_session", "id": id,
            "project_id": project, "agent_id": agent,
        }))
        .await
    }

    pub async fn send(&self, session: &str, version: u64, text: &str) -> Value {
        self.command(json!({
            "type": "send_message", "session_id": session,
            "expected_version": version, "text": text,
        }))
        .await
    }
}

impl Drop for Workspace {
    fn drop(&mut self) {
        // Assertion failures must not leave a listening server behind.
        if let Some(server) = &self.server {
            server.abort();
        }
    }
}

pub fn success(output: &Output) -> Value {
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert!(output.stderr.is_empty(), "{output:?}");
    let response: Value = serde_json::from_slice(&output.stdout).expect("JSON stdout");
    assert_eq!(response["api_version"], 1);
    assert_eq!(response["ok"], true, "{response}");
    assert!(response.get("error").is_none(), "{response}");
    assert!(response["result"]["kind"].is_string(), "{response}");
    response["result"]["value"].clone()
}

pub fn failure(output: &Output, code: &str) {
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    assert!(output.stderr.is_empty(), "{output:?}");
    let response: Value = serde_json::from_slice(&output.stdout).expect("JSON error stdout");
    assert_eq!(response["api_version"], 1);
    assert_eq!(response["ok"], false);
    assert_eq!(response["error"]["code"], code);
    assert!(
        response["error"]["message"]
            .as_str()
            .is_some_and(|text| !text.is_empty())
    );
    assert_eq!(response["error"]["retryable"], false);
    assert!(response.get("result").is_none());
}

pub fn entity<'a>(workspace: &'a Value, collection: &str, id: &Value) -> &'a Value {
    workspace[collection]
        .as_array()
        .unwrap()
        .iter()
        .find(|entity| &entity["id"] == id)
        .unwrap_or_else(|| panic!("missing {collection} {id}: {workspace}"))
}

pub fn events(output: &Output) -> Vec<(u64, Value)> {
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert!(output.stderr.is_empty(), "{output:?}");
    let text = std::str::from_utf8(&output.stdout).unwrap();
    text.split("\n\n")
        .filter(|frame| !frame.trim().is_empty())
        .map(|frame| {
            let field = |prefix| {
                frame
                    .lines()
                    .find_map(|line| line.strip_prefix(prefix))
                    .unwrap()
            };
            let cursor: u64 = field("id:").trim().parse().unwrap();
            let event: Value = serde_json::from_str(field("data:").trim()).unwrap();
            assert_eq!(event["cursor"], cursor);
            assert_eq!(event["kind"], field("event:").trim());
            assert_eq!(event["api_version"], 1);
            (cursor, event)
        })
        .collect()
}
