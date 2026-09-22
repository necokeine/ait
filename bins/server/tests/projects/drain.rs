use std::path::Path;
use std::sync::mpsc;
use std::time::Duration;

use futures_util::SinkExt;
use serde_json::json;
use server_api::Api;
use server_application::Projects;
use server_domain::ProjectId;
use server_ports::{Catalog, CatalogEntry, OpenIntent, ProjectError, Receipt};
use server_storage::{SqliteCatalog, SqliteProjects};
use tokio_tungstenite::tungstenite::Message;

use super::fixture::Fixture;
use super::transport::{TOKEN, request, socket};

#[derive(Debug)]
struct PausedCatalog {
    inner: SqliteCatalog,
    entered: Option<tokio::sync::oneshot::Sender<()>>,
    release: mpsc::Receiver<()>,
}

impl Catalog for PausedCatalog {
    fn begin_open(&mut self, key: &str, path: &Path) -> Result<OpenIntent, ProjectError> {
        self.inner.begin_open(key, path)
    }
    fn finish_open(
        &mut self,
        intent: &OpenIntent,
        entry: &CatalogEntry,
    ) -> Result<Receipt, ProjectError> {
        if let Some(entered) = self.entered.take() {
            entered.send(()).unwrap();
            self.release.recv_timeout(Duration::from_secs(10)).unwrap();
        }
        self.inner.finish_open(intent, entry)
    }
    fn get(&mut self, id: ProjectId) -> Result<CatalogEntry, ProjectError> {
        self.inner.get(id)
    }
    fn list(
        &mut self,
        after: Option<ProjectId>,
        limit: usize,
    ) -> Result<Vec<CatalogEntry>, ProjectError> {
        self.inner.list(after, limit)
    }
    fn close_receipt(&mut self, key: &str, id: ProjectId) -> Result<Option<Receipt>, ProjectError> {
        self.inner.close_receipt(key, id)
    }
    fn finish_close(&mut self, key: &str, id: ProjectId) -> Result<Receipt, ProjectError> {
        self.inner.finish_close(key, id)
    }
}

#[tokio::test]
async fn accepted_blocking_job_survives_disconnect_and_is_included_in_drain() {
    let fixture = Fixture::new();
    let (entered, signal) = tokio::sync::oneshot::channel();
    let (release, gate) = mpsc::channel();
    let catalog = PausedCatalog {
        inner: SqliteCatalog::open(&fixture.state("state")).unwrap(),
        entered: Some(entered),
        release: gate,
    };
    let projects = Projects::new(
        Box::new(catalog),
        Box::new(SqliteProjects),
        Box::new(fixture.workspace()),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let api = Api::new(
        address,
        "s".to_owned(),
        "i".to_owned(),
        TOKEN.into(),
        server_api::Services {
            projects: Some(projects),
            agents: Some(server_application::agents::Agents::new(Box::new(
                SqliteCatalog::open(&fixture.state("state")).unwrap(),
            ))),
            daemon: None,
            directory: None,
        },
    )
    .unwrap();
    let shutdown = api.clone();
    let router = api.router();
    let server = tokio::spawn(async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(async move { shutdown.wait_draining().await })
            .await
            .unwrap();
    });
    let mut client = socket(address, server_protocol::project_lease::CAPABILITIES).await;
    client.send(Message::Text(json!({"type":"request","request_id":"lost-response","method":"project.open","params":{"path":fixture.repo,"idempotency_key":"survive"}}).to_string().into())).await.unwrap();
    tokio::time::timeout(Duration::from_secs(10), signal)
        .await
        .unwrap()
        .unwrap();
    let mut observer = socket(address, server_protocol::project_lease::CAPABILITIES).await;
    assert_eq!(
        request(&mut observer, "project.list", json!({})).await["code"],
        "resource_exhausted"
    );
    let mut agent_client = socket(address, server_protocol::agent::CAPABILITIES).await;
    assert_eq!(
        request(&mut agent_client, "agent.list", json!({})).await["code"],
        "resource_exhausted"
    );
    drop(agent_client);
    drop(client);
    api.begin_shutdown();
    assert!(
        tokio::time::timeout(Duration::from_millis(30), api.wait_closed())
            .await
            .is_err()
    );
    release.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        server.await.unwrap();
        api.wait_closed().await;
    })
    .await
    .unwrap();
    drop(api);
    let mut restarted = fixture.application("state");
    let receipt = restarted.open(&fixture.repo, "survive").unwrap();
    assert!(
        restarted
            .get(receipt.project_id)
            .unwrap()
            .owner_epoch
            .is_none()
    );
    assert_eq!(restarted.list(None, 20).unwrap().len(), 1);
}
