use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use crate::ports::agent_runtime::{AgentRuntimeRegistry, AgentRuntimeRegistryError};
use crate::ports::agent_session::{AgentSessionFuture, AgentSessionSpec};
use server_domain::agent_runtime::{AgentPersistenceHandle, StoredAgentConfig};

use super::*;

#[derive(Debug, Default)]
struct RegistryState {
    records: BTreeMap<String, PersistedAgentRuntimeRecord>,
    fail_upsert: bool,
    fail_update: bool,
}

#[derive(Debug, Clone, Default)]
struct MemoryRegistry(Arc<Mutex<RegistryState>>);

impl AgentRuntimeRegistry for MemoryRegistry {
    fn initialize(&self) -> Result<(), AgentRuntimeRegistryError> {
        Ok(())
    }

    fn list(&self) -> Result<Vec<PersistedAgentRuntimeRecord>, AgentRuntimeRegistryError> {
        Ok(self
            .0
            .lock()
            .expect("registry")
            .records
            .values()
            .cloned()
            .collect())
    }

    fn get(
        &self,
        id: &str,
    ) -> Result<Option<PersistedAgentRuntimeRecord>, AgentRuntimeRegistryError> {
        Ok(self.0.lock().expect("registry").records.get(id).cloned())
    }

    fn upsert(
        &self,
        record: &PersistedAgentRuntimeRecord,
    ) -> Result<(), AgentRuntimeRegistryError> {
        let mut state = self.0.lock().expect("registry");
        if state.fail_upsert {
            return Err(AgentRuntimeRegistryError::Io);
        }
        state.records.insert(record.id.clone(), record.clone());
        Ok(())
    }

    fn update(
        &self,
        id: &str,
        update: &dyn Fn(&PersistedAgentRuntimeRecord) -> PersistedAgentRuntimeRecord,
    ) -> Result<Option<PersistedAgentRuntimeRecord>, AgentRuntimeRegistryError> {
        let mut state = self.0.lock().expect("registry");
        let Some(current) = state.records.get(id) else {
            return Ok(None);
        };
        if state.fail_update {
            return Err(AgentRuntimeRegistryError::Io);
        }
        let next = update(current);
        state.records.insert(id.to_owned(), next.clone());
        Ok(Some(next))
    }

    fn remove(&self, id: &str) -> Result<bool, AgentRuntimeRegistryError> {
        Ok(self
            .0
            .lock()
            .expect("registry")
            .records
            .remove(id)
            .is_some())
    }
}

#[derive(Debug)]
struct FakeState {
    available: bool,
    fail_info: bool,
    fail_close: bool,
    handle_session_id: String,
    create_calls: usize,
    resume_purposes: Vec<AgentResumePurpose>,
    close_calls: usize,
    events: std::collections::VecDeque<AgentTurnEvent>,
    start_result: Result<(), AgentSessionError>,
    during_resume: Option<MemoryRegistry>,
}

impl Default for FakeState {
    fn default() -> Self {
        Self {
            available: true,
            fail_info: false,
            fail_close: false,
            handle_session_id: "native-1".to_owned(),
            create_calls: 0,
            resume_purposes: Vec::new(),
            close_calls: 0,
            events: std::collections::VecDeque::new(),
            start_result: Ok(()),
            during_resume: None,
        }
    }
}

#[derive(Debug, Clone, Default)]
struct FakeClient(Arc<Mutex<FakeState>>);

#[derive(Debug)]
struct FakeSession(Arc<Mutex<FakeState>>);

impl AgentClient for FakeClient {
    fn provider(&self) -> &'static str {
        "codex"
    }

    fn validate_config(&self, _: &StoredAgentConfig) -> Result<(), AgentSessionError> {
        Ok(())
    }

    fn is_available(&self) -> AgentSessionFuture<'_, bool> {
        Box::pin(async move { Ok(self.0.lock().expect("state").available) })
    }

    fn create_session<'a>(
        &'a self,
        _spec: &'a AgentSessionSpec,
    ) -> AgentSessionFuture<'a, Box<dyn AgentSession>> {
        Box::pin(async move {
            self.0.lock().expect("state").create_calls += 1;
            Ok(Box::new(FakeSession(self.0.clone())) as Box<dyn AgentSession>)
        })
    }

    fn resume_session<'a>(
        &'a self,
        _handle: &'a AgentPersistenceHandle,
        _spec: &'a AgentSessionSpec,
        purpose: AgentResumePurpose,
    ) -> AgentSessionFuture<'a, Box<dyn AgentSession>> {
        Box::pin(async move {
            self.0.lock().expect("state").resume_purposes.push(purpose);
            if let Some(registry) = &self.0.lock().expect("state").during_resume {
                registry
                    .update("agent-1", &|current| {
                        let mut next = current.clone();
                        next.title = Some("Changed during resume".to_owned());
                        next.requires_attention = true;
                        next
                    })
                    .unwrap();
            }
            Ok(Box::new(FakeSession(self.0.clone())) as Box<dyn AgentSession>)
        })
    }
}

impl AgentSession for FakeSession {
    fn start_turn<'a>(
        &'a mut self,
        _text: &'a str,
        _config: &'a server_domain::agent_runtime::StoredAgentConfig,
    ) -> AgentSessionFuture<'a, String> {
        Box::pin(async move {
            self.0.lock().unwrap().start_result?;
            Ok("native-turn".to_owned())
        })
    }

    fn cancel_turn<'a>(&'a mut self, _turn_id: &'a str) -> AgentSessionFuture<'a, ()> {
        Box::pin(async move {
            self.0
                .lock()
                .unwrap()
                .events
                .push_back(AgentTurnEvent::Cancelled);
            Ok(())
        })
    }

    fn poll_turn(&mut self) -> Result<Option<AgentTurnEvent>, AgentSessionError> {
        Ok(self.0.lock().unwrap().events.pop_front())
    }
    fn provider(&self) -> &'static str {
        "codex"
    }

    fn runtime_info(&mut self) -> AgentSessionFuture<'_, StoredAgentRuntimeInfo> {
        Box::pin(async move {
            let state = self.0.lock().expect("state");
            if state.fail_info {
                return Err(AgentSessionError::Failed);
            }
            Ok(StoredAgentRuntimeInfo {
                provider: "codex".to_owned(),
                session_id: Some("native-1".to_owned()),
                model: Some("gpt-6".to_owned()),
                thinking_option_id: None,
                mode_id: None,
                extra: None,
            })
        })
    }

    fn persistence(&self) -> Option<AgentPersistenceHandle> {
        Some(AgentPersistenceHandle {
            provider: "codex".to_owned(),
            session_id: self.0.lock().expect("state").handle_session_id.clone(),
            native_handle: None,
            metadata: None,
        })
    }

    fn close(&mut self) -> AgentSessionFuture<'_, ()> {
        Box::pin(async move {
            let mut state = self.0.lock().expect("state");
            state.close_calls += 1;
            if state.fail_close {
                Err(AgentSessionError::Failed)
            } else {
                Ok(())
            }
        })
    }
}

fn spec() -> AgentSessionSpec {
    AgentSessionSpec {
        provider: "codex".to_owned(),
        cwd: "/tmp/project".to_owned(),
        config: StoredAgentConfig {
            model: Some("gpt-6".to_owned()),
            ..StoredAgentConfig::default()
        },
    }
}

fn make_manager() -> (AgentManager, MemoryRegistry, FakeClient) {
    let registry = MemoryRegistry::default();
    let client = FakeClient::default();
    let mut manager = AgentManager::new(Box::new(registry.clone()));
    manager
        .register_client(Box::new(client.clone()))
        .expect("register client");
    (manager, registry, client)
}

fn stored_record() -> PersistedAgentRuntimeRecord {
    PersistedAgentRuntimeRecord {
        id: "agent-1".to_owned(),
        provider: "codex".to_owned(),
        cwd: "/tmp/project".to_owned(),
        workspace_id: Some("workspace-1".to_owned()),
        created_at: "2026-09-20T10:00:00.000Z".to_owned(),
        updated_at: "2026-09-20T10:01:00.000Z".to_owned(),
        last_activity_at: Some("2026-09-20T10:02:00.000Z".to_owned()),
        last_user_message_at: None,
        title: Some("Existing".to_owned()),
        labels: BTreeMap::new(),
        last_status: AgentRuntimeStatus::Closed,
        last_mode_id: None,
        config: Some(spec().config),
        runtime_info: None,
        features: Vec::new(),
        persistence: Some(AgentPersistenceHandle {
            provider: "codex".to_owned(),
            session_id: "native-1".to_owned(),
            native_handle: None,
            metadata: None,
        }),
        last_error: None,
        requires_attention: true,
        attention_reason: None,
        attention_timestamp: None,
        internal: false,
        archived_at: None,
        owner: None,
    }
}

#[tokio::test]
async fn create_registers_live_session_and_durable_snapshot() {
    let (mut manager, registry, client) = make_manager();
    let registration = AgentRegistration {
        workspace_id: Some("workspace-1".to_owned()),
        title: Some("New".to_owned()),
        ..AgentRegistration::default()
    };

    let created = manager
        .create("agent-1", &spec(), registration)
        .await
        .expect("create");

    assert_eq!(created.last_status, AgentRuntimeStatus::Idle);
    assert_eq!(created.title.as_deref(), Some("New"));
    assert_eq!(
        created
            .persistence
            .as_ref()
            .map(|handle| handle.session_id.as_str()),
        Some("native-1")
    );
    assert_eq!(registry.get("agent-1").expect("get"), Some(created));
    assert!(manager.live_snapshot("agent-1").is_some());
    assert_eq!(client.0.lock().expect("state").create_calls, 1);
}

#[tokio::test]
async fn duplicate_and_invalid_id_do_not_launch_provider() {
    let (mut manager, registry, client) = make_manager();
    registry.upsert(&stored_record()).expect("seed");

    assert_eq!(
        manager
            .create("agent-1", &spec(), AgentRegistration::default())
            .await,
        Err(AgentManagerError::AlreadyExists("agent-1".to_owned()))
    );
    assert_eq!(
        manager
            .create("", &spec(), AgentRegistration::default())
            .await,
        Err(AgentManagerError::InvalidRequest)
    );
    assert_eq!(client.0.lock().expect("state").create_calls, 0);
}

#[tokio::test]
async fn unavailable_provider_does_not_create_session() {
    let (mut manager, _, client) = make_manager();
    client.0.lock().expect("state").available = false;

    assert_eq!(
        manager
            .create("agent-1", &spec(), AgentRegistration::default())
            .await,
        Err(AgentManagerError::ProviderUnavailable("codex".to_owned()))
    );
    assert_eq!(client.0.lock().expect("state").create_calls, 0);
}

#[tokio::test]
async fn failed_inspection_or_mismatched_handle_closes_unregistered_session() {
    let (mut manager, registry, client) = make_manager();
    client.0.lock().expect("state").fail_info = true;
    assert_eq!(
        manager
            .create("agent-1", &spec(), AgentRegistration::default())
            .await,
        Err(AgentManagerError::Session)
    );
    assert_eq!(client.0.lock().expect("state").close_calls, 1);
    assert!(registry.get("agent-1").expect("get").is_none());

    let (mut manager, _, client) = make_manager();
    client.0.lock().expect("state").handle_session_id = "wrong".to_owned();
    assert_eq!(
        manager
            .create("agent-2", &spec(), AgentRegistration::default())
            .await,
        Err(AgentManagerError::Session)
    );
    assert_eq!(client.0.lock().expect("state").close_calls, 1);
}

#[tokio::test]
async fn failed_persistence_closes_session_and_reports_registry_error() {
    let (mut manager, registry, client) = make_manager();
    registry.0.lock().expect("registry").fail_upsert = true;

    assert_eq!(
        manager
            .create("agent-1", &spec(), AgentRegistration::default())
            .await,
        Err(AgentManagerError::Registry)
    );
    assert_eq!(client.0.lock().expect("state").close_calls, 1);
    assert!(manager.live_snapshot("agent-1").is_none());
}

#[tokio::test]
async fn failed_close_keeps_ownership_for_retry() {
    let (mut manager, registry, client) = make_manager();
    manager
        .create("agent-1", &spec(), AgentRegistration::default())
        .await
        .expect("create");
    client.0.lock().expect("state").fail_close = true;

    assert_eq!(
        manager.close("agent-1").await,
        Err(AgentManagerError::Session)
    );
    assert!(manager.live_snapshot("agent-1").is_some());
    assert_eq!(
        registry
            .get("agent-1")
            .expect("get")
            .expect("record")
            .last_status,
        AgentRuntimeStatus::Idle
    );
    client.0.lock().expect("state").fail_close = false;
    manager.close("agent-1").await.expect("retry close");
    assert!(manager.live_snapshot("agent-1").is_none());
    assert_eq!(
        registry
            .get("agent-1")
            .expect("get")
            .expect("record")
            .last_status,
        AgentRuntimeStatus::Closed
    );
}

#[tokio::test]
async fn archived_resume_uses_history_purpose_and_preserves_metadata() {
    let (mut manager, registry, client) = make_manager();
    let mut record = stored_record();
    record.archived_at = Some("2026-09-21T10:00:00.000Z".to_owned());
    registry.upsert(&record).expect("seed");

    let resumed = manager.resume("agent-1").await.expect("resume");

    assert_eq!(
        client.0.lock().expect("state").resume_purposes,
        vec![AgentResumePurpose::History]
    );
    assert_eq!(resumed.created_at, record.created_at);
    assert_eq!(resumed.updated_at, record.updated_at);
    assert_eq!(resumed.last_activity_at, record.last_activity_at);
    assert_eq!(resumed.archived_at, record.archived_at);
    assert!(resumed.requires_attention);
    assert_eq!(
        manager.resume("agent-1").await.expect("idempotent"),
        resumed
    );
    assert_eq!(client.0.lock().expect("state").resume_purposes.len(), 1);
}

#[tokio::test]
async fn active_resume_is_interactive_and_missing_handle_is_rejected() {
    let (mut manager, registry, client) = make_manager();
    registry.upsert(&stored_record()).expect("seed");
    manager.resume("agent-1").await.expect("resume");
    assert_eq!(
        client.0.lock().expect("state").resume_purposes,
        vec![AgentResumePurpose::Interactive]
    );

    let mut without_handle = stored_record();
    without_handle.id = "agent-2".to_owned();
    without_handle.persistence = None;
    registry.upsert(&without_handle).expect("seed");
    assert_eq!(
        manager.resume("agent-2").await,
        Err(AgentManagerError::MissingPersistence("agent-2".to_owned()))
    );
    assert_eq!(
        manager.resume("missing").await,
        Err(AgentManagerError::NotFound("missing".to_owned()))
    );
}

#[tokio::test]
async fn failed_cleanup_retains_live_session_until_close_succeeds() {
    let (mut manager, registry, client) = make_manager();
    {
        let mut state = client.0.lock().expect("state");
        state.fail_info = true;
        state.fail_close = true;
    }

    assert_eq!(
        manager
            .create("agent-1", &spec(), AgentRegistration::default())
            .await,
        Err(AgentManagerError::Session)
    );
    assert!(manager.live.contains_key("agent-1"));
    assert_eq!(
        manager.resume("agent-1").await,
        Err(AgentManagerError::Session)
    );
    client.0.lock().expect("state").fail_close = false;
    manager
        .close("agent-1")
        .await
        .expect("close retained runtime");
    assert!(manager.live_snapshot("agent-1").is_none());
    assert!(registry.get("agent-1").expect("get").is_none());
}

#[test]
fn duplicate_provider_registration_is_rejected() {
    let (mut manager, _, client) = make_manager();

    assert_eq!(
        manager.register_client(Box::new(client)),
        Err(AgentManagerError::AlreadyExists("codex".to_owned()))
    );
}

#[tokio::test]
async fn close_all_attempts_every_live_session() {
    let (mut manager, registry, client) = make_manager();
    manager
        .create("agent-1", &spec(), AgentRegistration::default())
        .await
        .expect("first");
    manager
        .create("agent-2", &spec(), AgentRegistration::default())
        .await
        .expect("second");

    manager.close_all().await.expect("close all");

    assert_eq!(client.0.lock().expect("state").close_calls, 2);
    assert!(manager.live.is_empty());
    assert!(
        registry
            .list()
            .expect("list")
            .iter()
            .all(|record| { record.last_status == AgentRuntimeStatus::Closed })
    );
}

#[tokio::test]
async fn terminal_write_failures_retain_work_and_close_preserves_concurrent_metadata() {
    let (mut manager, registry, client) = make_manager();
    let connection = manager.events().connect();
    let notifications = Arc::new(Mutex::new(Vec::new()));
    let sink = notifications.clone();
    let subscription = connection
        .subscribe(
            server_metadata::protocol::session::EventsRequest {
                events: vec!["agent_attention_required".to_owned()],
                notifications: false,
            },
            Arc::new(move |_, payload| {
                sink.lock().unwrap().push(payload);
                Ok(())
            }),
        )
        .unwrap();
    subscription.activate().unwrap();
    manager
        .create("agent-1", &spec(), AgentRegistration::default())
        .await
        .unwrap();
    manager.send("agent-1", "hello").await.unwrap();
    manager.poll().await.unwrap();
    client
        .0
        .lock()
        .unwrap()
        .events
        .push_back(AgentTurnEvent::Completed(Some("answer".to_owned())));
    registry.0.lock().unwrap().fail_update = true;
    assert_eq!(manager.poll().await, Err(AgentManagerError::Registry));
    assert!(notifications.lock().unwrap().is_empty());
    assert_eq!(manager.active_turn("agent-1"), Some("native-turn"));
    assert_eq!(manager.last_message("agent-1"), None);
    registry.0.lock().unwrap().fail_update = false;
    registry
        .update("agent-1", &|current| {
            let mut next = current.clone();
            next.title = Some("Newest title".to_owned());
            next.labels.insert("new".to_owned(), "label".to_owned());
            next
        })
        .unwrap();
    manager.poll().await.unwrap();
    manager.poll().await.unwrap();
    assert_eq!(notifications.lock().unwrap().len(), 1);
    assert_eq!(notifications.lock().unwrap()[0]["reason"], "finished");
    assert_eq!(manager.last_message("agent-1"), Some("answer"));
    assert_eq!(manager.active_turn("agent-1"), None);
    registry.0.lock().unwrap().fail_update = true;
    assert_eq!(
        manager.close("agent-1").await,
        Err(AgentManagerError::Registry)
    );
    assert!(manager.live_snapshot("agent-1").is_some());
    registry.0.lock().unwrap().fail_update = false;
    manager.close("agent-1").await.unwrap();
    let latest = registry.get("agent-1").unwrap().unwrap();
    assert_eq!(latest.title.as_deref(), Some("Newest title"));
    assert_eq!(latest.labels["new"], "label");
}

#[tokio::test]
async fn accepted_turn_survives_runtime_write_failure_and_reports_inspection_failure() {
    let (mut manager, registry, client) = make_manager();
    manager
        .create("agent-1", &spec(), AgentRegistration::default())
        .await
        .unwrap();
    manager.send("agent-1", "hello").await.unwrap();
    registry.0.lock().unwrap().fail_update = true;
    assert_eq!(manager.poll().await, Err(AgentManagerError::Registry));
    assert_eq!(manager.active_turn("agent-1"), Some("native-turn"));
    registry.0.lock().unwrap().fail_update = false;
    manager.poll().await.unwrap();
    assert_eq!(manager.active_turn("agent-1"), Some("native-turn"));
    assert!(manager.live["agent-1"].pending_runtime.is_none());
    manager.cancel("agent-1").await.unwrap();
    manager.poll().await.unwrap();
    client.0.lock().unwrap().fail_info = true;
    manager
        .send("agent-1", "second accepted turn")
        .await
        .unwrap();
    assert_eq!(manager.active_turn("agent-1"), Some("native-turn"));
    manager.poll().await.unwrap();
    assert!(manager.live_snapshot("agent-1").is_none());
    assert_eq!(
        registry.get("agent-1").unwrap().unwrap().last_status,
        AgentRuntimeStatus::Error
    );
    assert_eq!(client.0.lock().unwrap().close_calls, 1);
}

#[tokio::test]
async fn cancelled_internal_archived_and_deleted_agents_do_not_publish_attention() {
    for outcome in ["cancelled", "internal", "archived", "deleted"] {
        let (mut manager, registry, client) = make_manager();
        manager
            .create(
                "agent-1",
                &spec(),
                AgentRegistration {
                    internal: outcome == "internal",
                    ..AgentRegistration::default()
                },
            )
            .await
            .unwrap();
        manager.send("agent-1", "hello").await.unwrap();
        manager.poll().await.unwrap();
        let connection = manager.events().connect();
        let events = Arc::new(Mutex::new(Vec::new()));
        let captured = events.clone();
        let subscription = connection
            .subscribe(
                server_metadata::protocol::session::EventsRequest {
                    events: vec!["agent_attention_required".to_owned()],
                    notifications: false,
                },
                Arc::new(move |_, payload| {
                    captured.lock().unwrap().push(payload);
                    Ok(())
                }),
            )
            .unwrap();
        subscription.activate().unwrap();
        if outcome == "archived" {
            registry
                .update("agent-1", &|record| {
                    let mut next = record.clone();
                    next.archived_at = Some("2026-09-24T00:00:00Z".to_owned());
                    next
                })
                .unwrap();
        } else if outcome == "deleted" {
            registry.remove("agent-1").unwrap();
        }
        client
            .0
            .lock()
            .unwrap()
            .events
            .push_back(if outcome == "cancelled" {
                AgentTurnEvent::Cancelled
            } else {
                AgentTurnEvent::Completed(None)
            });
        manager.poll().await.unwrap();
        assert!(events.lock().unwrap().is_empty(), "{outcome}");
        assert_eq!(manager.active_turn("agent-1"), None);
        if outcome == "deleted" {
            assert!(registry.get("agent-1").unwrap().is_none());
        }
        manager.close_all().await.unwrap();
    }
}

#[tokio::test]
async fn configuration_write_failure_preserves_the_previous_bundle_and_unrelated_metadata() {
    let (manager, registry, _) = make_manager();
    registry.upsert(&stored_record()).unwrap();
    let original = registry.get("agent-1").unwrap().unwrap();
    let patch = ConfigPatch {
        model_id: crate::protocol::agent_config::NullableSetting::Set("new".to_owned()),
        thinking_option_id: crate::protocol::agent_config::NullableSetting::Set("high".to_owned()),
        ..ConfigPatch::default()
    };
    registry.0.lock().unwrap().fail_update = true;
    assert_eq!(
        manager.configure("agent-1", &patch).await,
        Err(AgentManagerError::Registry)
    );
    assert_eq!(registry.get("agent-1").unwrap().unwrap(), original);
    registry.0.lock().unwrap().fail_update = false;
    manager.configure("agent-1", &patch).await.unwrap();
    let updated = registry.get("agent-1").unwrap().unwrap();
    assert_eq!(updated.config.unwrap().model.as_deref(), Some("new"));
    assert_eq!(updated.title, original.title);
    assert_eq!(updated.persistence, original.persistence);
    assert!(matches!(
        manager.configure("missing", &patch).await,
        Err(AgentManagerError::NotFound(_))
    ));
    let mut unsupported = original;
    unsupported.provider = "unknown".to_owned();
    registry.upsert(&unsupported).unwrap();
    assert!(matches!(
        manager.configure("agent-1", &patch).await,
        Err(AgentManagerError::ProviderUnavailable(_))
    ));
}

#[tokio::test]
async fn resume_merges_runtime_facts_without_overwriting_changes_during_native_io() {
    let (mut manager, registry, client) = make_manager();
    registry.upsert(&stored_record()).unwrap();
    client.0.lock().unwrap().during_resume = Some(registry.clone());
    let resumed = manager.resume("agent-1").await.unwrap();
    assert_eq!(resumed.title.as_deref(), Some("Changed during resume"));
    assert!(resumed.requires_attention);
    manager.close_all().await.unwrap();
}

#[tokio::test]
async fn failed_start_is_persisted_and_native_ownership_is_released() {
    let (mut manager, registry, client) = make_manager();
    manager
        .create("agent-1", &spec(), AgentRegistration::default())
        .await
        .unwrap();
    client.0.lock().unwrap().start_result = Err(AgentSessionError::Failed);
    assert_eq!(
        manager.send("agent-1", "hello").await,
        Err(AgentManagerError::Session)
    );
    assert_eq!(
        registry.get("agent-1").unwrap().unwrap().last_status,
        AgentRuntimeStatus::Error
    );
    assert!(manager.live_snapshot("agent-1").is_none());
    assert_eq!(client.0.lock().unwrap().close_calls, 1);
}

#[tokio::test]
async fn live_session_admission_is_bounded() {
    let (mut manager, registry, client) = make_manager();
    for index in 0..32 {
        manager
            .create(
                &format!("agent-{index}"),
                &spec(),
                AgentRegistration::default(),
            )
            .await
            .unwrap();
    }
    assert_eq!(
        manager
            .create("overflow", &spec(), AgentRegistration::default())
            .await,
        Err(AgentManagerError::Busy)
    );
    let mut stored = stored_record();
    stored.id = "stored-overflow".to_owned();
    registry.upsert(&stored).unwrap();
    assert_eq!(
        manager.resume("stored-overflow").await,
        Err(AgentManagerError::Busy)
    );
    assert_eq!(client.0.lock().unwrap().create_calls, 32);
    manager.close_all().await.unwrap();
}
