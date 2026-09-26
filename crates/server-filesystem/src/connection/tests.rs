//! Paseo owned-subscriptions and file-explorer observer contracts at dispatch boundaries.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};
use server_model::outbound::{Frame, Outbound, Queued};
use server_model::{Context, Lifecycle, Limits, Request, Runtime, ServerInfo};
use tokio::sync::mpsc;

use super::Connection;
use crate::capabilities::Group;
use crate::dispatch::{State, dispatch};
use crate::local::{checkout::LocalCheckout, files::LocalFiles};
use crate::service::{checkout::Checkout, files::Files};

struct Harness {
    root: Arc<tempfile::TempDir>,
    state: Arc<State>,
    connection: Connection,
    outbound: Outbound,
    receiver: mpsc::Receiver<Queued>,
    available: usize,
}

impl Harness {
    fn new() -> Self {
        let root = Arc::new(tempfile::tempdir().unwrap());
        let state = Arc::new(State {
            runtime: Arc::new(Runtime::new(ServerInfo {
                server_id: "test-server".to_owned(),
                instance_id: "test-instance".to_owned(),
                listen: "127.0.0.1:0".to_owned(),
                lifecycle: Lifecycle::Ready,
                protocol: server_model::server::VERSION,
                capabilities: Vec::new(),
                implemented_capabilities: Vec::new(),
                limits: Limits::default(),
            })),
            files: Some(Arc::new(Mutex::new(Files::new(Box::new(LocalFiles::new(
                root.path().to_owned(),
                &root.path().join("data"),
            )))))),
            checkout: Some(Arc::new(Mutex::new(Checkout::new(Box::new(
                LocalCheckout::new(root.path().join("managed")),
            ))))),
            skills: None,
            forge: None,
            github_projects: None,
            worktrees: None,
            workspace_recovery: None,
            workspace_automation: None,
        });
        let (outbound, receiver) = Outbound::new();
        Self {
            root,
            state,
            connection: Connection::default(),
            outbound,
            receiver,
            available: 10,
        }
    }

    fn peer(&self) -> Self {
        let (outbound, receiver) = Outbound::new();
        Self {
            root: self.root.clone(),
            state: self.state.clone(),
            connection: Connection::default(),
            outbound,
            receiver,
            available: 10,
        }
    }

    fn cwd(&self) -> &str {
        self.root.path().to_str().unwrap()
    }

    async fn request(&mut self, method: &str, params: Value) -> Value {
        let group = if method.starts_with("checkout.") {
            Group::Checkout
        } else {
            Group::Files
        };
        dispatch(
            group,
            Context {
                request: Request {
                    id: "request".to_owned(),
                    method: method.to_owned(),
                    params,
                },
                runtime: &self.state.runtime,
                outbound: &self.outbound,
                available_subscriptions: self.available,
            },
            &self.state,
            &mut self.connection,
        )
        .await
        .unwrap();
        let response = self.receive().await;
        assert_eq!(
            response["request_id"], "request",
            "unexpected event before request response: {response}"
        );
        response
    }

    async fn subscribe_file(&mut self, path: &str, id: Option<&str>) -> String {
        let response = self
            .request(
                "fs.file.subscribe.request",
                json!({"cwd":self.cwd(),"path":path,"subscriptionId":id}),
            )
            .await;
        assert_eq!(response["type"], "response", "{response}");
        response["result"]["subscriptionId"]
            .as_str()
            .unwrap()
            .to_owned()
    }

    async fn subscribe_diff(&mut self) -> String {
        let response = self
            .request(
                "checkout.diff.subscribe.request",
                json!({"cwd":self.cwd(),"compare":{"mode":"uncommitted"}}),
            )
            .await;
        assert_eq!(response["type"], "response", "{response}");
        response["result"]["subscriptionId"]
            .as_str()
            .unwrap()
            .to_owned()
    }

    async fn receive(&mut self) -> Value {
        let frame = tokio::time::timeout(Duration::from_secs(5), self.receiver.recv())
            .await
            .unwrap()
            .unwrap();
        let Frame::Text(text) = frame.message else {
            panic!("expected JSON frame")
        };
        serde_json::from_str(&text).unwrap()
    }

    async fn quiet(&mut self) {
        assert!(
            tokio::time::timeout(Duration::from_millis(450), self.receiver.recv())
                .await
                .is_err()
        );
    }

    async fn finish(mut self) {
        self.connection = Connection::default();
        self.state.cancellation.cancel();
        self.state.tasks.close();
        tokio::time::timeout(Duration::from_secs(5), self.state.tasks.wait())
            .await
            .unwrap();
    }

    fn initialize_git(&self) {
        for arguments in [
            vec!["init", "--quiet"],
            vec!["config", "user.name", "Test"],
            vec!["config", "user.email", "test@example.invalid"],
            vec![
                "-c",
                "commit.gpgsign=false",
                "commit",
                "--allow-empty",
                "-m",
                "base",
            ],
        ] {
            let output = std::process::Command::new("git")
                .args(&arguments)
                .current_dir(self.root.path())
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "git {arguments:?}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }
}

#[tokio::test]
async fn repeated_file_query_has_independent_ids_and_independent_release() {
    let mut harness = Harness::new();
    std::fs::write(harness.root.path().join("file"), "before").unwrap();
    let first = harness.subscribe_file("file", None).await;
    let second = harness.subscribe_file("file", None).await;
    assert_ne!(first, second);
    assert_eq!(harness.connection.len(), 2);
    std::fs::write(harness.root.path().join("file"), "changed").unwrap();
    let observed = BTreeSet::from([
        harness.receive().await["params"]["subscriptionId"]
            .as_str()
            .unwrap()
            .to_owned(),
        harness.receive().await["params"]["subscriptionId"]
            .as_str()
            .unwrap()
            .to_owned(),
    ]);
    assert_eq!(observed, BTreeSet::from([first.clone(), second.clone()]));
    harness.connection.release(&first);
    std::fs::write(harness.root.path().join("file"), "changed again").unwrap();
    assert_eq!(harness.receive().await["params"]["subscriptionId"], second);
    harness.quiet().await;
    harness.finish().await;
}

#[tokio::test]
async fn identical_subscription_ids_on_separate_connections_have_separate_owners() {
    let mut first = Harness::new();
    let mut second = first.peer();
    std::fs::write(first.root.path().join("file"), "before").unwrap();
    first.subscribe_file("file", Some("same-id")).await;
    second.subscribe_file("file", Some("same-id")).await;
    first.connection.release("same-id");
    std::fs::write(first.root.path().join("file"), "after").unwrap();
    assert_eq!(
        second.receive().await["params"]["subscriptionId"],
        "same-id"
    );
    first.quiet().await;
    drop(second);
    first.finish().await;
}

#[tokio::test]
async fn file_deletion_and_recreation_keep_the_observation_alive() {
    let mut harness = Harness::new();
    std::fs::write(harness.root.path().join("file"), "before").unwrap();
    let id = harness.subscribe_file("file", None).await;
    std::fs::remove_file(harness.root.path().join("file")).unwrap();
    let missing = harness.receive().await;
    assert_eq!(missing["params"]["subscriptionId"], id);
    assert_eq!(missing["params"]["version"]["status"], "missing");
    std::fs::write(harness.root.path().join("file"), "recreated").unwrap();
    let ready = harness.receive().await;
    assert_eq!(ready["params"]["subscriptionId"], id);
    assert_eq!(ready["params"]["version"]["status"], "ready");
    assert_eq!(harness.connection.len(), 1);
    harness.finish().await;
}

#[tokio::test]
async fn file_observers_publish_each_subscribers_own_path_coordinates() {
    let mut harness = Harness::new();
    std::fs::create_dir(harness.root.path().join("nested")).unwrap();
    std::fs::write(harness.root.path().join("nested/file"), "before").unwrap();
    let first = harness
        .subscribe_file("nested/file", Some("root-view"))
        .await;
    let nested = harness.root.path().join("nested");
    let response = harness
        .request(
            "fs.file.subscribe.request",
            json!({"cwd":nested,"path":"file","subscriptionId":"nested-view"}),
        )
        .await;
    assert_eq!(response["type"], "response");
    std::fs::write(harness.root.path().join("nested/file"), "after").unwrap();
    for _ in 0..2 {
        let event = harness.receive().await;
        if event["params"]["subscriptionId"] == first {
            assert_eq!(event["params"]["version"]["cwd"], harness.cwd());
            assert_eq!(event["params"]["version"]["path"], "nested/file");
        } else {
            assert_eq!(event["params"]["subscriptionId"], "nested-view");
            assert_eq!(event["params"]["version"]["cwd"], nested.to_str().unwrap());
            assert_eq!(event["params"]["version"]["path"], "file");
        }
    }
    harness.finish().await;
}

#[tokio::test]
async fn explicit_file_id_replacement_is_allowed_at_capacity_and_stops_the_old_path() {
    let mut harness = Harness::new();
    for path in ["old", "new"] {
        std::fs::write(harness.root.path().join(path), "before").unwrap();
    }
    harness.subscribe_file("old", Some("replace")).await;
    harness.available = 0;
    harness.subscribe_file("new", Some("replace")).await;
    assert_eq!(harness.connection.len(), 1);
    std::fs::write(harness.root.path().join("old"), "unobserved").unwrap();
    harness.quiet().await;
    std::fs::write(harness.root.path().join("new"), "observed").unwrap();
    let event = harness.receive().await;
    assert_eq!(event["params"]["subscriptionId"], "replace");
    assert_eq!(event["params"]["version"]["path"], "new");
    harness.finish().await;
}

#[tokio::test]
async fn exhausted_file_subscription_budget_retains_existing_observers() {
    let mut harness = Harness::new();
    std::fs::write(harness.root.path().join("file"), "before").unwrap();
    let id = harness.subscribe_file("file", None).await;
    harness.available = 0;
    let rejected = harness
        .request(
            "fs.file.subscribe.request",
            json!({"cwd":harness.cwd(),"path":"file"}),
        )
        .await;
    assert_eq!(rejected["code"], "resource_exhausted");
    assert_eq!(harness.connection.len(), 1);
    std::fs::write(harness.root.path().join("file"), "after").unwrap();
    assert_eq!(harness.receive().await["params"]["subscriptionId"], id);
    harness.finish().await;
}

#[tokio::test]
async fn unchanged_file_is_quiet_and_releasing_last_observer_stops_polling() {
    let mut harness = Harness::new();
    std::fs::write(harness.root.path().join("file"), "before").unwrap();
    let id = harness.subscribe_file("file", None).await;
    harness.quiet().await;
    harness.connection.release(&id);
    std::fs::write(harness.root.path().join("file"), "after release").unwrap();
    harness.quiet().await;
    assert!(harness.connection.is_empty());
    harness.state.tasks.close();
    tokio::time::timeout(Duration::from_secs(5), harness.state.tasks.wait())
        .await
        .unwrap();
    harness.finish().await;
}

#[tokio::test]
async fn repeated_diff_queries_keep_independent_observations_after_release() {
    let mut harness = Harness::new();
    harness.initialize_git();
    let first = harness.subscribe_diff().await;
    let second = harness.subscribe_diff().await;
    assert_ne!(first, second);
    assert_eq!(harness.connection.len(), 2);
    std::fs::write(harness.root.path().join("file"), "first change\n").unwrap();
    let observed = BTreeSet::from([
        harness.receive().await["params"]["subscriptionId"]
            .as_str()
            .unwrap()
            .to_owned(),
        harness.receive().await["params"]["subscriptionId"]
            .as_str()
            .unwrap()
            .to_owned(),
    ]);
    assert_eq!(observed, BTreeSet::from([first.clone(), second.clone()]));
    harness.connection.release(&first);
    std::fs::write(harness.root.path().join("file"), "second change\n").unwrap();
    let event = harness.receive().await;
    assert_eq!(event["method"], "checkout.diff.update");
    assert_eq!(event["params"]["subscriptionId"], second);
    harness.quiet().await;
    harness.finish().await;
}
