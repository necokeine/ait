//! Real daemon processes verify runtime lock release and HTTP ownership fencing.
use reqwest::Client;
use serde_json::{Value, json};
use std::{
    net::TcpListener,
    path::Path,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};
use tempfile::TempDir;

struct Daemon {
    child: Child,
    url: String,
    client: Client,
}
impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
impl Daemon {
    async fn start(database: &Path) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        drop(listener);
        let log_path = database.with_extension("log");
        let log = std::fs::File::create(&log_path).unwrap();
        let child = Command::new(env!("CARGO_BIN_EXE_ait-daemon"))
            .arg("--database")
            .arg(database)
            .arg("--listen")
            .arg(address.to_string())
            .stdout(Stdio::null())
            .stderr(Stdio::from(log))
            .spawn()
            .unwrap();
        let mut daemon = Self {
            child,
            url: format!("http://{address}"),
            client: Client::builder()
                .timeout(Duration::from_secs(2))
                .build()
                .unwrap(),
        };
        let deadline = Instant::now() + Duration::from_secs(30);
        while Instant::now() < deadline {
            if daemon
                .client
                .get(format!("{}/v1/project/list", daemon.url))
                .send()
                .await
                .is_ok()
            {
                return daemon;
            }
            assert!(
                daemon.child.try_wait().unwrap().is_none(),
                "daemon exited before readiness"
            );
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        panic!(
            "daemon readiness timeout: {}",
            std::fs::read_to_string(log_path).unwrap_or_default()
        );
    }
    async fn post(&self, route: &str, body: Value) -> Value {
        self.client
            .post(format!("{}{route}", self.url))
            .json(&body)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap()
    }
}
fn project(response: &Value) -> &Value {
    assert_eq!(response["ok"], true, "{response}");
    &response["result"]["value"]
}

async fn event_page(daemon: &Daemon, after: u64, namespace: &str) -> String {
    daemon
        .client
        .get(format!("{}/v1/event/list", daemon.url))
        .query(&[
            ("after", after.to_string()),
            ("namespace", namespace.into()),
        ])
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap()
}

fn last_event(page: &str) -> Value {
    serde_json::from_str(
        page.lines()
            .filter_map(|line| line.strip_prefix("data: "))
            .next_back()
            .expect("at least one durable event"),
    )
    .unwrap()
}

async fn assert_catalog_switch_resets_the_feed(a: &Daemon, b: &Daemon) {
    let original = last_event(&event_page(a, 0, "").await);
    let current = last_event(&event_page(b, 0, "").await);
    let namespace = current["namespace"].as_str().unwrap();
    let cursor = current["cursor"].as_u64().unwrap();
    assert_ne!(original["namespace"], current["namespace"]);
    assert!(event_page(b, cursor, namespace).await.is_empty());
    let reset = last_event(&event_page(b, cursor, original["namespace"].as_str().unwrap()).await);
    assert_eq!(reset["kind"], "stream.reset_required");
    assert_eq!(reset["namespace"], namespace);
}

#[cfg(unix)]
async fn assert_graceful_shutdown_releases_project(
    daemon: &mut Daemon,
    next_database: &Path,
    previous: &Value,
) {
    assert!(
        Command::new("/bin/kill")
            .arg("-TERM")
            .arg(daemon.child.id().to_string())
            .status()
            .unwrap()
            .success()
    );
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Some(status) = daemon.child.try_wait().unwrap() {
            assert!(status.success(), "daemon must finish graceful shutdown");
            break;
        }
        assert!(Instant::now() < deadline, "graceful shutdown timeout");
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    let next = Daemon::start(next_database).await;
    let opened = next
        .post(
            "/v1/project/register",
            json!({"id":"ignored","name":"Ignored","workdir":previous["workdir"]}),
        )
        .await;
    let opened = project(&opened);
    assert_eq!(opened["id"], previous["id"]);
    assert_eq!(opened["root_message_id"], previous["root_message_id"]);
    assert!(opened["owner"]["owner_epoch"].as_u64() > previous["owner"]["owner_epoch"].as_u64());
}

#[tokio::test]
async fn two_daemons_close_reopen_and_crash_takeover_preserve_identity() {
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("project");
    std::fs::create_dir(&root).unwrap();
    let id = format!(
        "portable-{}",
        temp.path().file_name().unwrap().to_str().unwrap()
    );
    let mut a = Daemon::start(&temp.path().join("a.sqlite3")).await;
    let mut b = Daemon::start(&temp.path().join("b.sqlite3")).await;
    let input = json!({"id":id,"name":"Stable","workdir":root});
    let registered = a.post("/v1/project/register", input.clone()).await;
    assert_eq!(project(&registered)["id"], id);
    let listed: Value = a
        .client
        .get(format!("{}/v1/project/list", a.url))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let original = listed["result"]["value"]
        .as_array()
        .unwrap()
        .iter()
        .find(|project| project["id"] == id)
        .unwrap()
        .clone();
    assert!(original["owner"].is_object(), "{listed}");
    let busy = b.post("/v1/project/register", input.clone()).await;
    assert_eq!(busy["ok"], false, "{busy}");
    assert!(busy.to_string().contains("PROJECT_BUSY"));
    project(&a.post("/v1/project/close", json!({"project_id":id})).await);
    let opened = b
        .post(
            "/v1/project/register",
            json!({"id":"ignored-new-id","name":"Ignored","workdir":root}),
        )
        .await;
    let opened = project(&opened);
    assert_eq!(opened["id"], id);
    assert_eq!(opened["root_message_id"], original["root_message_id"]);
    assert!(opened["owner"]["owner_epoch"].as_u64() > original["owner"]["owner_epoch"].as_u64());
    let stale: Value = b
        .client
        .post(format!("{}/v1/project/update", b.url))
        .header(
            "x-ait-project-owner",
            serde_json::to_string(&original["owner"]).unwrap(),
        )
        .json(&json!({"project_id":id,"name":"Stale"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(stale["ok"], false, "{stale}");
    assert_catalog_switch_resets_the_feed(&a, &b).await;
    let epoch = opened["owner"]["owner_epoch"].as_u64().unwrap();
    b.child.kill().unwrap();
    b.child.wait().unwrap();
    let reopened = a.post("/v1/project/register", input).await;
    let reopened = project(&reopened);
    assert_eq!(reopened["id"], id);
    assert!(reopened["owner"]["owner_epoch"].as_u64().unwrap() > epoch);
    assert_eq!(reopened["name"], "Stable");
    let history: Value = a
        .client
        .get(format!("{}/v1/message/list", a.url))
        .query(&[("project_id", &id)])
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(history["ok"], true, "{history}");
    assert_eq!(history["result"]["value"].as_array().unwrap().len(), 1);
    #[cfg(unix)]
    assert_graceful_shutdown_releases_project(&mut a, &temp.path().join("b.sqlite3"), reopened)
        .await;
}
