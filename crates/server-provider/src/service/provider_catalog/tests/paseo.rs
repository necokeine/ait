//! Paseo snapshot-manager cache, selective-refresh and failure-isolation contracts.

use std::sync::{Arc, Mutex};

use super::*;

#[derive(Debug)]
struct ProbeState {
    available: Result<bool, AgentSessionError>,
    discovery: Result<Details, AgentSessionError>,
    availability_calls: usize,
    discovery_cwds: Vec<String>,
}

#[derive(Debug, Clone)]
struct Probe {
    provider: &'static str,
    state: Arc<Mutex<ProbeState>>,
}

impl Probe {
    fn new(provider: &'static str) -> Self {
        Self {
            provider,
            state: Arc::new(Mutex::new(ProbeState {
                available: Ok(true),
                discovery: Ok(Details {
                    models: vec![
                        json!({"provider":provider,"id":"native-model","label":"Native model"}),
                    ],
                    modes: vec![json!({"id":"read-only","label":"Read only"})],
                    features: vec![json!({"id":"fast_mode","type":"toggle","value":false})],
                }),
                availability_calls: 0,
                discovery_cwds: Vec::new(),
            })),
        }
    }
}

impl AgentClient for Probe {
    fn provider(&self) -> &'static str {
        self.provider
    }

    fn is_available(&self) -> AgentSessionFuture<'_, bool> {
        Box::pin(async {
            let mut state = self.state.lock().unwrap();
            state.availability_calls += 1;
            state.available
        })
    }

    fn discover<'a>(&'a self, cwd: &'a str) -> AgentSessionFuture<'a, Details> {
        Box::pin(async {
            let mut state = self.state.lock().unwrap();
            state.discovery_cwds.push(cwd.to_owned());
            state.discovery.clone()
        })
    }

    fn create_session<'a>(
        &'a self,
        _: &'a AgentSessionSpec,
    ) -> AgentSessionFuture<'a, Box<dyn AgentSession>> {
        Box::pin(async { Err(AgentSessionError::Unavailable) })
    }

    fn resume_session<'a>(
        &'a self,
        _: &'a AgentPersistenceHandle,
        _: &'a AgentSessionSpec,
        _: AgentResumePurpose,
    ) -> AgentSessionFuture<'a, Box<dyn AgentSession>> {
        Box::pin(async { Err(AgentSessionError::Unavailable) })
    }
}

fn clients(probes: &[Probe]) -> BTreeMap<String, Box<dyn AgentClient>> {
    probes
        .iter()
        .map(|probe| {
            (
                probe.provider.to_owned(),
                Box::new(probe.clone()) as Box<dyn AgentClient>,
            )
        })
        .collect()
}

async fn snapshot(catalog: &mut Catalog, probes: &[Probe], cwd: &std::path::Path) -> Value {
    catalog
        .execute(
            &clients(probes),
            &SessionEvents::default(),
            "provider.snapshot.get.request",
            json!({"cwd":cwd}),
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn warm_snapshot_reads_do_not_probe_or_rediscover_the_provider() {
    let root = tempfile::tempdir().unwrap();
    let probe = Probe::new("codex");
    let mut catalog = Catalog::default();
    let first = snapshot(&mut catalog, std::slice::from_ref(&probe), root.path()).await;
    let second = snapshot(&mut catalog, std::slice::from_ref(&probe), root.path()).await;
    assert_eq!(first["snapshotHash"], second["snapshotHash"]);
    assert_eq!(first["entries"], second["entries"]);
    let state = probe.state.lock().unwrap();
    assert_eq!(state.availability_calls, 1);
    assert_eq!(state.discovery_cwds.len(), 1);
}

#[tokio::test]
async fn explicit_refresh_reprobes_only_the_selected_warm_provider() {
    let root = tempfile::tempdir().unwrap();
    let probes = [Probe::new("codex"), Probe::new("claude")];
    let mut catalog = Catalog::default();
    snapshot(&mut catalog, &probes, root.path()).await;
    catalog
        .execute(
            &clients(&probes),
            &SessionEvents::default(),
            "provider.snapshot.refresh.request",
            json!({"cwd":root.path(),"providers":["codex"]}),
        )
        .await
        .unwrap();
    assert_eq!(probes[0].state.lock().unwrap().availability_calls, 2);
    assert_eq!(probes[0].state.lock().unwrap().discovery_cwds.len(), 2);
    assert_eq!(probes[1].state.lock().unwrap().availability_calls, 1);
    assert_eq!(probes[1].state.lock().unwrap().discovery_cwds.len(), 1);
}

#[tokio::test]
async fn unavailable_provider_does_not_fetch_its_catalog() {
    let root = tempfile::tempdir().unwrap();
    let probe = Probe::new("codex");
    probe.state.lock().unwrap().available = Ok(false);
    let value = snapshot(
        &mut Catalog::default(),
        std::slice::from_ref(&probe),
        root.path(),
    )
    .await;
    assert_eq!(value["entries"][0]["status"], "unavailable");
    assert_eq!(value["entries"][0]["models"], json!([]));
    assert!(probe.state.lock().unwrap().discovery_cwds.is_empty());
}

#[tokio::test]
async fn failed_availability_probe_is_reported_without_attempting_discovery() {
    let root = tempfile::tempdir().unwrap();
    let probe = Probe::new("codex");
    probe.state.lock().unwrap().available = Err(AgentSessionError::Failed);
    let value = snapshot(
        &mut Catalog::default(),
        std::slice::from_ref(&probe),
        root.path(),
    )
    .await;
    assert_eq!(value["entries"][0]["status"], "unavailable");
    assert_eq!(
        value["entries"][0]["error"],
        "Provider executable is unavailable"
    );
    assert!(probe.state.lock().unwrap().discovery_cwds.is_empty());
}

#[tokio::test]
async fn one_provider_discovery_failure_keeps_other_provider_results() {
    let root = tempfile::tempdir().unwrap();
    let probes = [Probe::new("codex"), Probe::new("claude")];
    probes[0].state.lock().unwrap().discovery = Err(AgentSessionError::Failed);
    let value = snapshot(&mut Catalog::default(), &probes, root.path()).await;
    let entries = value["entries"].as_array().unwrap();
    let healthy = entries
        .iter()
        .find(|entry| entry["provider"] == "claude")
        .unwrap();
    let failed = entries
        .iter()
        .find(|entry| entry["provider"] == "codex")
        .unwrap();
    assert_eq!(healthy["status"], "ready");
    assert_eq!(healthy["models"][0]["id"], "native-model");
    assert_eq!(failed["status"], "error");
    assert_eq!(failed["error"], "Provider discovery failed");
}

#[tokio::test]
async fn unchanged_refresh_does_not_include_discovery_freshness_in_the_content_hash() {
    let root = tempfile::tempdir().unwrap();
    let probes = [Probe::new("codex")];
    let mut catalog = Catalog::default();
    let before = snapshot(&mut catalog, &probes, root.path()).await;
    catalog
        .execute(
            &clients(&probes),
            &SessionEvents::default(),
            "provider.snapshot.refresh.request",
            json!({"cwd":root.path()}),
        )
        .await
        .unwrap();
    let after = catalog
        .execute(
            &clients(&probes),
            &SessionEvents::default(),
            "provider.snapshot.get.request",
            json!({"cwd":root.path(),"ifNoneMatch":before["snapshotHash"]}),
        )
        .await
        .unwrap();
    assert_eq!(after["notModified"], true);
    assert_eq!(after["entries"], json!([]));
    assert_eq!(after["snapshotHash"], before["snapshotHash"]);
}

#[tokio::test]
async fn changed_native_model_content_invalidates_the_previous_snapshot_hash() {
    let root = tempfile::tempdir().unwrap();
    let probes = [Probe::new("codex")];
    let mut catalog = Catalog::default();
    let before = snapshot(&mut catalog, &probes, root.path()).await;
    probes[0]
        .state
        .lock()
        .unwrap()
        .discovery
        .as_mut()
        .unwrap()
        .models[0]["label"] = json!("Updated native label");
    catalog
        .execute(
            &clients(&probes),
            &SessionEvents::default(),
            "provider.snapshot.refresh.request",
            json!({"cwd":root.path()}),
        )
        .await
        .unwrap();
    let after = snapshot(&mut catalog, &probes, root.path()).await;
    assert_ne!(before["snapshotHash"], after["snapshotHash"]);
    assert_eq!(
        after["entries"][0]["models"][0]["label"],
        "Updated native label"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn workspace_aliases_return_the_same_canonical_cache_scope() {
    let root = tempfile::tempdir().unwrap();
    let actual = root.path().join("actual");
    let alias = root.path().join("alias");
    std::fs::create_dir(&actual).unwrap();
    std::os::unix::fs::symlink(&actual, &alias).unwrap();
    let probes = [Probe::new("codex")];
    let mut catalog = Catalog::default();
    let first = snapshot(&mut catalog, &probes, &actual).await;
    let second = snapshot(&mut catalog, &probes, &alias).await;
    assert_eq!(
        first["cwd"],
        actual.canonicalize().unwrap().to_str().unwrap()
    );
    assert_eq!(first["cwd"], second["cwd"]);
    assert_eq!(probes[0].state.lock().unwrap().discovery_cwds.len(), 1);
}

#[tokio::test]
async fn different_workspace_scopes_do_not_reuse_another_workspaces_catalog() {
    let first = tempfile::tempdir().unwrap();
    let second = tempfile::tempdir().unwrap();
    let probes = [Probe::new("codex")];
    let mut catalog = Catalog::default();
    snapshot(&mut catalog, &probes, first.path()).await;
    snapshot(&mut catalog, &probes, second.path()).await;
    let state = probes[0].state.lock().unwrap();
    assert_eq!(state.discovery_cwds.len(), 2);
    assert_ne!(state.discovery_cwds[0], state.discovery_cwds[1]);
}

#[tokio::test]
async fn expired_snapshot_reprobes_before_returning_new_native_content() {
    let root = tempfile::tempdir().unwrap();
    let probes = [Probe::new("codex")];
    let mut catalog = Catalog::default();
    snapshot(&mut catalog, &probes, root.path()).await;
    catalog.snapshots.values_mut().next().unwrap().fetched =
        Instant::now().checked_sub(Duration::from_secs(61)).unwrap();
    probes[0]
        .state
        .lock()
        .unwrap()
        .discovery
        .as_mut()
        .unwrap()
        .models[0]["id"] = json!("new-model");
    let refreshed = snapshot(&mut catalog, &probes, root.path()).await;
    assert_eq!(refreshed["entries"][0]["models"][0]["id"], "new-model");
    assert_eq!(probes[0].state.lock().unwrap().discovery_cwds.len(), 2);
}

#[tokio::test]
async fn directory_cache_evicts_the_oldest_scope_at_its_bound() {
    let root = tempfile::tempdir().unwrap();
    let probes = [Probe::new("codex")];
    let mut catalog = Catalog::default();
    for index in 0..17 {
        let cwd = root.path().join(index.to_string());
        std::fs::create_dir(&cwd).unwrap();
        snapshot(&mut catalog, &probes, &cwd).await;
    }
    assert_eq!(catalog.snapshots.len(), 16);
    let oldest = root.path().join("0").canonicalize().unwrap();
    assert!(!catalog.snapshots.contains_key(oldest.to_str().unwrap()));
    snapshot(&mut catalog, &probes, &oldest).await;
    assert_eq!(probes[0].state.lock().unwrap().discovery_cwds.len(), 18);
    assert_eq!(catalog.snapshots.len(), 16);
}

#[tokio::test]
async fn feature_discovery_failure_is_returned_inline_without_fabricated_features() {
    let root = tempfile::tempdir().unwrap();
    let probes = [Probe::new("codex")];
    probes[0].state.lock().unwrap().discovery = Err(AgentSessionError::Failed);
    let value = Catalog::default()
        .execute(
            &clients(&probes),
            &SessionEvents::default(),
            "provider.features.list.request",
            json!({"cwd":root.path(),"provider":"codex"}),
        )
        .await
        .unwrap();
    assert_eq!(value["features"], json!([]));
    assert_eq!(value["error"], "Provider discovery failed");
}
