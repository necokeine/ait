use super::*;
use crate::ports::agent_session::{
    AgentResumePurpose, AgentSession, AgentSessionError, AgentSessionFuture, AgentSessionSpec,
};
use server_domain::agent_runtime::AgentPersistenceHandle;

mod paseo;

#[derive(Debug)]
struct Client(bool);
impl AgentClient for Client {
    fn provider(&self) -> &'static str {
        "codex"
    }
    fn is_available(&self) -> AgentSessionFuture<'_, bool> {
        Box::pin(async move { Ok(self.0) })
    }
    fn discover<'a>(&'a self, _cwd: &'a str) -> AgentSessionFuture<'a, Details> {
        Box::pin(async {
            Ok(Details {
                models: vec![json!({"id":"test","provider":"codex","label":"Test"})],
                modes: vec![json!({"id":"read-only","label":"Read only"})],
                features: vec![],
            })
        })
    }
    fn create_session<'a>(
        &'a self,
        _spec: &'a AgentSessionSpec,
    ) -> AgentSessionFuture<'a, Box<dyn AgentSession>> {
        Box::pin(async { Err(AgentSessionError::Unavailable) })
    }
    fn resume_session<'a>(
        &'a self,
        _handle: &'a AgentPersistenceHandle,
        _spec: &'a AgentSessionSpec,
        _purpose: AgentResumePurpose,
    ) -> AgentSessionFuture<'a, Box<dyn AgentSession>> {
        Box::pin(async { Err(AgentSessionError::Unavailable) })
    }
}

#[tokio::test]
async fn catalog_uses_registered_adapters_scopes_cache_and_honors_content_hashes() {
    let clients: BTreeMap<String, Box<dyn AgentClient>> = BTreeMap::from([(
        "codex".to_owned(),
        Box::new(Client(true)) as Box<dyn AgentClient>,
    )]);
    let mut catalog = Catalog::default();
    let events = SessionEvents::default();
    let cwd = tempfile::tempdir().unwrap();
    let first = catalog
        .execute(
            &clients,
            &events,
            "provider.snapshot.get.request",
            json!({"cwd":cwd.path()}),
        )
        .await
        .unwrap();
    assert_eq!(first["entries"][0]["status"], "ready");
    let unchanged = catalog
        .execute(
            &clients,
            &events,
            "provider.snapshot.get.request",
            json!({"cwd":cwd.path(),"ifNoneMatch":first["snapshotHash"]}),
        )
        .await
        .unwrap();
    assert_eq!(unchanged["notModified"], true);
    assert_eq!(unchanged["entries"], json!([]));
    for (method, field) in [
        ("provider.models.list.request", "models"),
        ("provider.modes.list.request", "modes"),
        ("provider.features.list.request", "features"),
    ] {
        let value = catalog
            .execute(
                &clients,
                &events,
                method,
                json!({"provider":"codex","cwd":cwd.path()}),
            )
            .await
            .unwrap();
        assert!(value[field].is_array());
    }
    assert_eq!(
        catalog
            .execute(
                &clients,
                &events,
                "provider.snapshot.refresh.request",
                json!({"cwd":cwd.path(),"providers":["codex"]})
            )
            .await
            .unwrap()["acknowledged"],
        true
    );
    assert!(
        catalog
            .execute(
                &clients,
                &events,
                "provider.snapshot.refresh.request",
                json!({"providers":["missing"]})
            )
            .await
            .is_err()
    );
    assert!(
        catalog
            .execute(
                &clients,
                &events,
                "provider.models.list.request",
                json!({"provider":"missing"})
            )
            .await
            .is_err()
    );
    assert!(
        catalog
            .execute(
                &clients,
                &events,
                "provider.snapshot.get.request",
                json!({"cwd":"relative"})
            )
            .await
            .is_err()
    );
    assert!(
        catalog
            .execute(
                &clients,
                &events,
                "provider.snapshot.get.request",
                json!({"cwd":cwd.path().join("missing")})
            )
            .await
            .is_err()
    );
}

#[tokio::test]
async fn unavailable_provider_is_reported_truthfully() {
    let clients: BTreeMap<String, Box<dyn AgentClient>> = BTreeMap::from([(
        "codex".to_owned(),
        Box::new(Client(false)) as Box<dyn AgentClient>,
    )]);
    let value = Catalog::default()
        .execute(
            &clients,
            &SessionEvents::default(),
            "provider.available.list.request",
            json!({}),
        )
        .await
        .unwrap();
    assert_eq!(value["providers"][0]["available"], false);
    assert_eq!(
        value["providers"][0]["error"],
        "Provider executable is unavailable"
    );
}
