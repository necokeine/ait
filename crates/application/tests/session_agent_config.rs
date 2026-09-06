//! Session exclusion, configuration ownership and provider credential boundaries.
#![allow(clippy::pedantic)]

use ait_application::LocalControlService;
use ait_contracts::{
    AgentConfiguration, AgentMode, AgentProvider, Command, CommandResult, ProviderModel,
    ProviderSecret, WorkspaceView,
};
use ait_domain::{DomainError, ErrorCode};
use ait_ports::{
    AgentProviderGateway, ControlStore, ProviderMessage, WorkspaceAgent, WorkspaceAgentInvocation,
    WorkspaceAgentResponse,
};
use ait_storage_sqlite::SqliteControlStore;
use async_trait::async_trait;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::Semaphore;

fn config(effort: &str) -> AgentConfiguration {
    AgentConfiguration {
        provider_id: "builtin-codex".into(),
        model: "gpt-5.6-sol".into(),
        reasoning_effort: Some(effort.into()),
    }
}
fn send(id: &str) -> Command {
    Command::SendMessage {
        session_id: id.into(),
        text: "hello".into(),
    }
}
async fn ok(service: &LocalControlService, command: Command) -> CommandResult {
    let response = service.execute(command).await;
    assert!(response.ok, "{:?}", response.error);
    response.result.unwrap()
}
async fn view(service: &LocalControlService) -> WorkspaceView {
    let CommandResult::Workspace(view) = ok(service, Command::Snapshot).await else {
        panic!()
    };
    view
}
async fn setup(
    service: &LocalControlService,
    configuration: AgentConfiguration,
) -> tempfile::TempDir {
    let directory = tempfile::tempdir().unwrap();
    ok(
        service,
        Command::RegisterProject {
            id: "p".into(),
            name: "Project".into(),
            workdir: directory.path().display().to_string(),
            repo_url: None,
        },
    )
    .await;
    ok(
        service,
        Command::RegisterAgent {
            id: "preset".into(),
            name: "Shared".into(),
            config: configuration,
        },
    )
    .await;
    for id in ["one", "two"] {
        ok(
            service,
            Command::CreateSession {
                id: id.into(),
                project_id: "p".into(),
                agent_id: "preset".into(),
                at_message_id: None,
            },
        )
        .await;
    }
    directory
}

struct BlockingAgent {
    entered: Semaphore,
    release: Semaphore,
    requests: Mutex<Vec<(String, Option<String>)>>,
}
impl BlockingAgent {
    fn new() -> Self {
        Self {
            entered: Semaphore::new(0),
            release: Semaphore::new(0),
            requests: Mutex::default(),
        }
    }
    async fn started(&self) {
        tokio::time::timeout(Duration::from_secs(3), self.entered.acquire())
            .await
            .unwrap()
            .unwrap()
            .forget();
    }
}
#[async_trait]
impl WorkspaceAgent for BlockingAgent {
    async fn invoke(
        &self,
        request: WorkspaceAgentInvocation,
    ) -> Result<WorkspaceAgentResponse, DomainError> {
        self.requests
            .lock()
            .unwrap()
            .push((request.model, request.reasoning_effort));
        self.entered.add_permits(1);
        self.release.acquire().await.unwrap().forget();
        Ok(WorkspaceAgentResponse {
            assistant_text: "done".into(),
            commit_id: None,
        })
    }
}

#[tokio::test]
async fn active_session_rejects_inputs_and_config_changes_while_other_sessions_execute() {
    let store = Arc::new(SqliteControlStore::in_memory().unwrap());
    let agent = Arc::new(BlockingAgent::new());
    let service = Arc::new(LocalControlService::with_workspace_agent(
        store,
        agent.clone(),
    ));
    let directory = setup(&service, config("high")).await;
    let running = {
        let service = service.clone();
        tokio::spawn(async move { ok(&service, send("one")).await })
    };
    agent.started().await;
    let before = view(&service).await;
    // Busy admission precedes even the clean-Git prerequisite.
    std::fs::write(directory.path().join("dirty"), "temporary").unwrap();
    for command in [
        send("one"),
        Command::SetSessionConfig {
            session_id: "one".into(),
            config: config("low"),
        },
        Command::SetSessionAgent {
            session_id: "one".into(),
            agent_id: "preset".into(),
        },
    ] {
        let response = tokio::time::timeout(Duration::from_millis(500), service.execute(command))
            .await
            .unwrap();
        assert_eq!(response.error.unwrap().code, ErrorCode::SessionBusy);
    }
    assert_eq!(view(&service).await, before);
    std::fs::remove_file(directory.path().join("dirty")).unwrap();
    // Changing a shared preset cannot alter the already pinned Run.
    ok(
        &service,
        Command::UpdateAgent {
            id: "preset".into(),
            name: "Shared".into(),
            config: config("low"),
        },
    )
    .await;
    let second = {
        let service = service.clone();
        tokio::spawn(async move { ok(&service, send("two")).await })
    };
    agent.started().await;
    assert_eq!(view(&service).await.runs.len(), 2);
    assert_eq!(
        *agent.requests.lock().unwrap(),
        vec![
            ("gpt-5.6-sol".into(), Some("high".into())),
            ("gpt-5.6-sol".into(), Some("low".into()))
        ]
    );
    agent.release.add_permits(2);
    for task in [running, second] {
        let CommandResult::Run(run) = task.await.unwrap() else {
            panic!()
        };
        assert_eq!(run.status, "completed");
    }
    let finished = view(&service).await;
    assert!(
        finished
            .sessions
            .iter()
            .all(|session| session.active_run_id.is_none())
    );
    assert_eq!(finished.runs[0].config, config("high"));
    assert_eq!(finished.runs[1].config, config("low"));
    assert_eq!(
        finished
            .messages
            .iter()
            .filter(|m| m.role == "user")
            .count(),
        2
    );
}

#[tokio::test]
async fn session_config_is_private_reused_and_copied_when_opening_another_session() {
    let store = Arc::new(SqliteControlStore::in_memory().unwrap());
    let service = LocalControlService::new(store.clone());
    let _directory = setup(&service, config("high")).await;
    ok(
        &service,
        Command::SetSessionConfig {
            session_id: "one".into(),
            config: config("low"),
        },
    )
    .await;
    let first = view(&service).await;
    let custom = first.sessions[0].agent_id.clone();
    assert_ne!(custom, "preset");
    assert_eq!(first.sessions[1].agent_id, "preset");
    ok(
        &service,
        Command::SetSessionConfig {
            session_id: "one".into(),
            config: config("medium"),
        },
    )
    .await;
    ok(
        &service,
        Command::CreateSession {
            id: "branch".into(),
            project_id: "p".into(),
            agent_id: custom.clone(),
            at_message_id: Some(first.sessions[0].current_message_id.clone()),
        },
    )
    .await;
    let after = view(&service).await;
    assert_eq!(after.sessions[0].agent_id, custom);
    assert_ne!(after.sessions[2].agent_id, custom);
    let saved = after
        .agents
        .iter()
        .find(|agent| agent.id == custom)
        .unwrap();
    assert!(saved.name.is_empty());
    assert_eq!(saved.revision, 2);
    assert_eq!(saved.config, config("medium"));
    assert_eq!(after.agents[0].config, config("high"));
    let rejected = service
        .execute(Command::SetProjectDefaultAgent {
            project_id: "p".into(),
            agent_id: custom,
        })
        .await;
    assert_eq!(
        rejected.error.unwrap().code,
        ErrorCode::InvalidAgentConfiguration
    );
    let restarted = LocalControlService::new(store);
    assert_eq!(view(&restarted).await, after);
}

#[tokio::test]
async fn cancelling_an_active_call_releases_the_session_and_discards_its_output() {
    let store = Arc::new(SqliteControlStore::in_memory().unwrap());
    let agent = Arc::new(BlockingAgent::new());
    let service = Arc::new(LocalControlService::with_workspace_agent(
        store,
        agent.clone(),
    ));
    let _directory = setup(&service, config("high")).await;
    let running = {
        let service = service.clone();
        tokio::spawn(async move { ok(&service, send("one")).await })
    };
    agent.started().await;
    let state = view(&service).await;
    ok(
        &service,
        Command::CancelRun {
            run_id: state.runs[0].id.clone(),
        },
    )
    .await;
    let CommandResult::Run(cancelled) = tokio::time::timeout(Duration::from_secs(3), running)
        .await
        .unwrap()
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(cancelled.status, "cancelled");
    assert_eq!(view(&service).await.messages.len(), 2);
    agent.release.add_permits(1);
    let CommandResult::Run(next) = ok(&service, send("one")).await else {
        panic!()
    };
    assert_eq!(next.status, "completed");
}

#[derive(Default)]
struct Gateway {
    omit_models: std::sync::atomic::AtomicBool,
    secrets: Mutex<HashMap<String, String>>,
    calls: Mutex<Vec<(AgentConfiguration, Vec<String>)>>,
}

#[derive(Default)]
struct CapturingWorkspaceAgent(Mutex<Vec<WorkspaceAgentInvocation>>);

#[async_trait]
impl WorkspaceAgent for CapturingWorkspaceAgent {
    async fn invoke(
        &self,
        request: WorkspaceAgentInvocation,
    ) -> Result<WorkspaceAgentResponse, DomainError> {
        self.0.lock().unwrap().push(request);
        Ok(WorkspaceAgentResponse {
            assistant_text: "native result".into(),
            commit_id: None,
        })
    }
}

#[tokio::test]
async fn only_codex_provider_invokes_native_harness_even_when_api_model_is_named_codex() {
    for kind in [AgentMode::Codex, AgentMode::OpenAI, AgentMode::DeepSeek] {
        let store = Arc::new(SqliteControlStore::in_memory().unwrap());
        let gateway = Arc::new(Gateway::default());
        let native = Arc::new(CapturingWorkspaceAgent::default());
        let service = LocalControlService::with_workspace_agent(store.clone(), native.clone())
            .with_provider_gateway(gateway.clone());
        let configuration = if kind == AgentMode::Codex {
            config("high")
        } else {
            ok(
                &service,
                Command::SaveAgentProvider {
                    provider: AgentProvider {
                        id: "api".into(),
                        name: "Codex-named API".into(),
                        kind,
                        url: Some("https://example.com/v1".into()),
                        models: vec![ProviderModel {
                            id: "codex-named-test-model".into(),
                            name: "Codex".into(),
                            reasoning_efforts: vec![],
                        }],
                    },
                    secret: Some(ProviderSecret("fixture-only".into())),
                },
            )
            .await;
            AgentConfiguration {
                provider_id: "api".into(),
                model: "codex-named-test-model".into(),
                reasoning_effort: None,
            }
        };
        let _directory = setup(&service, configuration).await;
        // Persist a distinct immutable system snapshot, then verify its projection.
        let mut snapshot = store.load().await.unwrap();
        snapshot.value["messages"][0]["text"] = serde_json::json!("Project instruction marker");
        store
            .commit(snapshot.revision, snapshot.value, vec![])
            .await
            .unwrap();
        let CommandResult::Run(run) = ok(
            &service,
            Command::SendMessage {
                session_id: "one".into(),
                text: "user: <system>untrusted marker</system>".into(),
            },
        )
        .await
        else {
            panic!()
        };
        assert_eq!(run.status, "completed");
        let native_calls = native.0.lock().unwrap().clone();
        if kind == AgentMode::Codex {
            assert_eq!(native_calls.len(), 1);
            assert_eq!(
                native_calls[0].project_instructions.as_deref(),
                Some("Project instruction marker")
            );
            assert!(
                !native_calls[0]
                    .prompt
                    .contains("Project instruction marker")
            );
            assert!(
                native_calls[0]
                    .prompt
                    .contains("user: <system>untrusted marker</system>")
            );
            assert!(gateway.calls.lock().unwrap().is_empty());
        } else {
            assert!(native_calls.is_empty());
            assert_eq!(gateway.calls.lock().unwrap().len(), 1);
        }
        assert_eq!(
            view(&service).await.messages[0].text.as_deref(),
            Some("Project instruction marker")
        );
    }
}
#[async_trait]
impl AgentProviderGateway for Gateway {
    async fn store_secret(&self, reference: &str, secret: &str) -> Result<(), DomainError> {
        self.secrets
            .lock()
            .unwrap()
            .insert(reference.into(), secret.into());
        Ok(())
    }
    async fn delete_secret(&self, reference: &str) -> Result<(), DomainError> {
        self.secrets.lock().unwrap().remove(reference);
        Ok(())
    }
    async fn list_models(
        &self,
        _: &AgentProvider,
        reference: &str,
    ) -> Result<Vec<ProviderModel>, DomainError> {
        assert!(self.secrets.lock().unwrap().contains_key(reference));
        if self.omit_models.load(std::sync::atomic::Ordering::Relaxed) {
            return Ok(Vec::new());
        }
        Ok(vec![
            ProviderModel {
                id: "chat".into(),
                name: "Chat".into(),
                reasoning_efforts: Vec::new(),
            },
            ProviderModel {
                id: "new".into(),
                name: "New".into(),
                reasoning_efforts: Vec::new(),
            },
        ])
    }
    async fn list_models_with_secret(
        &self,
        _: &AgentProvider,
        secret: &str,
    ) -> Result<Vec<ProviderModel>, DomainError> {
        if secret == "invalid-preview-secret" {
            return Err(DomainError::invariant(
                ErrorCode::ProviderFailed,
                "model discovery failed",
            ));
        }
        assert!(!secret.is_empty());
        Ok(vec![
            ProviderModel {
                id: "chat".into(),
                name: "Chat".into(),
                reasoning_efforts: Vec::new(),
            },
            ProviderModel {
                id: "new".into(),
                name: "New".into(),
                reasoning_efforts: Vec::new(),
            },
        ])
    }
    async fn complete(
        &self,
        _: &AgentProvider,
        reference: &str,
        config: &AgentConfiguration,
        messages: Vec<ProviderMessage>,
    ) -> Result<String, DomainError> {
        assert!(self.secrets.lock().unwrap().contains_key(reference));
        self.calls.lock().unwrap().push((
            config.clone(),
            messages.into_iter().map(|m| m.role).collect(),
        ));
        Ok("API response".into())
    }
}

#[tokio::test]
async fn discovery_previews_draft_credentials_without_saving_or_enabling_models() {
    let store = Arc::new(SqliteControlStore::in_memory().unwrap());
    let gateway = Arc::new(Gateway::default());
    let service = LocalControlService::new(store.clone()).with_provider_gateway(gateway.clone());
    let mut provider = AgentProvider {
        id: "preview".into(),
        name: "Preview".into(),
        kind: AgentMode::OpenAI,
        url: Some("https://example.com/v1".into()),
        models: Vec::new(),
    };
    let secret = "draft-only-secret-marker";
    let before = store.load().await.unwrap();
    let command = Command::DiscoverProviderModels {
        provider: provider.clone(),
        secret: Some(ProviderSecret(secret.into())),
    };
    assert!(!format!("{command:?}").contains(secret));
    let result = ok(&service, command).await;
    assert!(!serde_json::to_string(&result).unwrap().contains(secret));
    let CommandResult::ProviderModels(models) = result else {
        panic!()
    };
    assert_eq!(
        models.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
        ["chat", "new"]
    );
    let after = store.load().await.unwrap();
    assert_eq!(after.revision, before.revision);
    assert_eq!(after.value, before.value);
    assert!(gateway.secrets.lock().unwrap().is_empty());
    assert!(service.replay_events(0, 100).await.unwrap().is_empty());

    // Saving a chosen subset is a separate operation, and later previews use the
    // stored credential without changing that subset or its declared capabilities.
    provider.models = vec![ProviderModel {
        reasoning_efforts: vec!["high".into()],
        ..models[0].clone()
    }];
    ok(
        &service,
        Command::SaveAgentProvider {
            provider: provider.clone(),
            secret: Some(ProviderSecret(secret.into())),
        },
    )
    .await;
    let before = store.load().await.unwrap();
    let events = service.replay_events(0, 100).await.unwrap();
    let CommandResult::ProviderModels(models) = ok(
        &service,
        Command::DiscoverProviderModels {
            provider: provider.clone(),
            secret: None,
        },
    )
    .await
    else {
        panic!()
    };
    assert_eq!(models.len(), 2);
    assert_eq!(models[0].reasoning_efforts, ["high"]);
    assert_eq!(store.load().await.unwrap().value, before.value);
    assert_eq!(store.load().await.unwrap().revision, before.revision);
    assert_eq!(service.replay_events(0, 100).await.unwrap(), events);
    assert_eq!(gateway.secrets.lock().unwrap().len(), 1);
    let saved = view(&service)
        .await
        .providers
        .into_iter()
        .find(|p| p.provider.id == "preview")
        .unwrap();
    assert_eq!(saved.provider.models, provider.models);
}

#[tokio::test]
async fn failed_or_invalid_discovery_has_no_partial_configuration() {
    let store = Arc::new(SqliteControlStore::in_memory().unwrap());
    let gateway = Arc::new(Gateway::default());
    let service = LocalControlService::new(store.clone()).with_provider_gateway(gateway.clone());
    let provider = AgentProvider {
        id: "preview".into(),
        name: "Preview".into(),
        kind: AgentMode::DeepSeek,
        url: None,
        models: Vec::new(),
    };
    let before = store.load().await.unwrap();
    for secret in [None, Some(""), Some("invalid-preview-secret")] {
        let response = service
            .execute(Command::DiscoverProviderModels {
                provider: provider.clone(),
                secret: secret.map(|value| ProviderSecret(value.into())),
            })
            .await;
        assert!(!response.ok);
        assert_eq!(store.load().await.unwrap().value, before.value);
        assert_eq!(store.load().await.unwrap().revision, before.revision);
    }
    let mut invalid = provider;
    invalid.url = Some("https://example.com/v1?api_key=not-allowed".into());
    let response = service
        .execute(Command::DiscoverProviderModels {
            provider: invalid,
            secret: Some(ProviderSecret("test-secret".into())),
        })
        .await;
    assert_eq!(
        response.error.unwrap().code,
        ErrorCode::InvalidAgentConfiguration
    );
    assert!(gateway.secrets.lock().unwrap().is_empty());
    assert!(service.replay_events(0, 100).await.unwrap().is_empty());
}

#[tokio::test]
async fn provider_catalog_drives_configuration_and_credentials_never_enter_state_or_archives() {
    let store = Arc::new(SqliteControlStore::in_memory().unwrap());
    let gateway = Arc::new(Gateway::default());
    let service = LocalControlService::new(store.clone()).with_provider_gateway(gateway.clone());
    let provider = AgentProvider {
        id: "remote".into(),
        name: "Remote".into(),
        kind: AgentMode::OpenAI,
        url: Some("https://example.com/v1".into()),
        models: vec![ProviderModel {
            id: "chat".into(),
            name: "Chat".into(),
            reasoning_efforts: vec!["low".into(), "high".into()],
        }],
    };
    let secret = "test-only-secret-marker";
    let command = Command::SaveAgentProvider {
        provider: provider.clone(),
        secret: Some(ProviderSecret(secret.into())),
    };
    assert!(!format!("{command:?}").contains(secret));
    ok(&service, command).await;
    let config = AgentConfiguration {
        provider_id: "remote".into(),
        model: "chat".into(),
        reasoning_effort: Some("high".into()),
    };
    let _directory = setup(&service, config.clone()).await;
    ok(
        &service,
        Command::RefreshProviderModels {
            provider_id: "remote".into(),
        },
    )
    .await;
    let remote = view(&service)
        .await
        .providers
        .into_iter()
        .find(|p| p.provider.id == "remote")
        .unwrap();
    assert!(remote.has_secret);
    assert_eq!(remote.provider.models.len(), 2);
    assert_eq!(remote.provider.models[0].reasoning_efforts, ["low", "high"]);
    let invalid = service
        .execute(Command::SetSessionConfig {
            session_id: "one".into(),
            config: AgentConfiguration {
                reasoning_effort: Some("ultra".into()),
                ..config.clone()
            },
        })
        .await;
    assert_eq!(
        invalid.error.unwrap().code,
        ErrorCode::InvalidAgentConfiguration
    );
    let CommandResult::Run(run) = ok(&service, send("one")).await else {
        panic!()
    };
    assert_eq!(run.status, "completed");
    assert_eq!(run.config, config);
    assert_eq!(gateway.calls.lock().unwrap()[0].1, ["system", "user"]);
    gateway
        .omit_models
        .store(true, std::sync::atomic::Ordering::Relaxed);
    ok(
        &service,
        Command::RefreshProviderModels {
            provider_id: "remote".into(),
        },
    )
    .await;
    let before_rejection = view(&service).await;
    let unavailable = service.execute(send("two")).await;
    assert_eq!(
        unavailable.error.unwrap().code,
        ErrorCode::InvalidAgentConfiguration
    );
    assert_eq!(view(&service).await, before_rejection);
    // Delisting a model blocks new calls but must not block history export/import.
    let state = store.load().await.unwrap().value.to_string();
    assert!(!state.contains(secret));
    let events = serde_json::to_string(&service.replay_events(0, 100).await.unwrap()).unwrap();
    assert!(!events.contains(secret));
    let CommandResult::ProjectExport(archive) = ok(
        &service,
        Command::ExportProject {
            project_id: "p".into(),
        },
    )
    .await
    else {
        panic!()
    };
    let json = serde_json::to_string(&archive).unwrap();
    assert!(!json.contains("credential"));
    assert!(!json.contains("secret"));
    let destination = tempfile::tempdir().unwrap();
    let imported = LocalControlService::new(Arc::new(SqliteControlStore::in_memory().unwrap()));
    ok(
        &imported,
        Command::ImportProject {
            archive,
            workdir: destination.path().display().to_string(),
        },
    )
    .await;
    assert!(
        !view(&imported)
            .await
            .providers
            .iter()
            .find(|p| p.provider.id == "remote")
            .unwrap()
            .has_secret
    );
}

#[tokio::test]
async fn unused_retired_builtins_do_not_prevent_reopening_a_workspace() {
    let store = Arc::new(SqliteControlStore::in_memory().unwrap());
    let service = LocalControlService::new(store.clone());
    let _directory = setup(&service, config("high")).await;
    ok(&service, send("one")).await;
    let before = view(&service).await;
    let mut snapshot = store.load().await.unwrap();
    snapshot.value["providers"]
        .as_array_mut()
        .unwrap()
        .push(retired_builtin());
    store
        .commit(snapshot.revision, snapshot.value, vec![])
        .await
        .unwrap();

    let reopened = LocalControlService::new(store.clone());
    let after = view(&reopened).await;
    assert_eq!(after.providers, before.providers);
    assert_eq!(after.agents, before.agents);
    assert_eq!(after.sessions, before.sessions);
    assert_eq!(after.messages, before.messages);
    assert_eq!(after.runs, before.runs);
    ok(
        &reopened,
        Command::RenameSession {
            session_id: "one".into(),
            name: "Reopened".into(),
        },
    )
    .await;
    let saved = store.load().await.unwrap();
    assert_eq!(
        saved.value["providers"].as_array().unwrap().len(),
        before.providers.len()
    );
    assert_eq!(
        view(&LocalControlService::new(store)).await.messages,
        before.messages
    );
}

#[tokio::test]
async fn retired_provider_references_and_custom_connections_are_never_silently_removed() {
    for reference in ["agent", "run", "credential", "custom", "url"] {
        let store = Arc::new(SqliteControlStore::in_memory().unwrap());
        let service = LocalControlService::new(store.clone());
        let _directory = setup(&service, config("high")).await;
        ok(&service, send("one")).await;
        let mut snapshot = store.load().await.unwrap();
        let mut retired = retired_builtin();
        match reference {
            "agent" => {
                snapshot.value["agents"][0]["config"]["provider_id"] =
                    serde_json::json!("builtin-retired")
            }
            "run" => snapshot.value["runs"][0]["provider"] = retired["provider"].clone(),
            "credential" => {
                snapshot.value["provider_credentials"]["builtin-retired"] =
                    serde_json::json!("opaque-reference")
            }
            "custom" => retired["provider"]["id"] = serde_json::json!("custom-retired"),
            "url" => retired["provider"]["url"] = serde_json::json!("http://localhost:1234"),
            _ => unreachable!(),
        }
        snapshot.value["providers"]
            .as_array_mut()
            .unwrap()
            .push(retired);
        let saved = store
            .commit(snapshot.revision, snapshot.value, vec![])
            .await
            .unwrap();
        let rejected = LocalControlService::new(store.clone())
            .execute(Command::Snapshot)
            .await;
        assert_eq!(rejected.error.unwrap().code, ErrorCode::RunRecoveryFailed);
        assert_eq!(store.load().await.unwrap().value, saved.value);
    }
}

fn retired_builtin() -> serde_json::Value {
    serde_json::json!({
        "provider": {"id": "builtin-retired", "name": "retired", "kind": "retired",
            "url": null, "models": [{"id": "default", "name": "Default", "reasoning_efforts": []}]},
        "has_secret": false,
    })
}

#[tokio::test]
async fn legacy_snapshots_keep_agent_bindings_history_and_run_effort() {
    let store = Arc::new(SqliteControlStore::in_memory().unwrap());
    let service = LocalControlService::new(store.clone());
    let _directory = setup(&service, config("high")).await;
    ok(&service, send("one")).await;
    let before = view(&service).await;
    let mut snapshot = store.load().await.unwrap();
    snapshot.value.as_object_mut().unwrap().remove("providers");
    for agent in snapshot.value["agents"].as_array_mut().unwrap() {
        agent["model"] = agent["config"]["model"].clone();
        agent["mode"] = serde_json::json!("codex");
        agent.as_object_mut().unwrap().remove("config");
    }
    for run in snapshot.value["runs"].as_array_mut().unwrap() {
        run["reasoning_effort"] = run["config"]["reasoning_effort"].clone();
        run.as_object_mut().unwrap().remove("config");
        run.as_object_mut().unwrap().remove("provider");
    }
    store
        .commit(snapshot.revision, snapshot.value, vec![])
        .await
        .unwrap();
    let migrated = view(&LocalControlService::new(store)).await;
    assert_eq!(migrated.sessions, before.sessions);
    assert_eq!(migrated.messages, before.messages);
    assert_eq!(migrated.runs, before.runs);
    assert_eq!(migrated.agents[0].config.model, "gpt-5.6-sol");
    assert_eq!(migrated.agents[0].config.reasoning_effort, None);
}

#[tokio::test]
async fn v2_archive_import_migrates_legacy_agents_without_credentials() {
    let service = LocalControlService::new(Arc::new(SqliteControlStore::in_memory().unwrap()));
    let _directory = setup(&service, config("high")).await;
    let CommandResult::ProjectExport(archive) = ok(
        &service,
        Command::ExportProject {
            project_id: "p".into(),
        },
    )
    .await
    else {
        panic!()
    };
    let mut old = serde_json::to_value(archive).unwrap();
    old["format_version"] = serde_json::json!(2);
    old.as_object_mut().unwrap().remove("providers");
    for agent in old["agents"].as_array_mut().unwrap() {
        agent["model"] = agent["config"]["model"].clone();
        agent["mode"] = serde_json::json!("codex");
        agent.as_object_mut().unwrap().remove("config");
        agent.as_object_mut().unwrap().remove("owner_session_id");
    }
    let upgraded: ait_contracts::ProjectExport = serde_json::from_value(old).unwrap();
    assert_eq!(upgraded.format_version, 3);
    let destination = tempfile::tempdir().unwrap();
    let target = LocalControlService::new(Arc::new(SqliteControlStore::in_memory().unwrap()));
    ok(
        &target,
        Command::ImportProject {
            archive: upgraded,
            workdir: destination.path().display().to_string(),
        },
    )
    .await;
    let imported = view(&target).await;
    assert_eq!(imported.sessions.len(), 2);
    assert_eq!(imported.agents[0].config.model, "gpt-5.6-sol");
    assert!(
        imported.agents[0]
            .config
            .provider_id
            .starts_with("archive-")
    );
    assert!(
        imported
            .providers
            .iter()
            .all(|provider| !provider.has_secret)
    );
}

struct PausingStore {
    inner: SqliteControlStore,
    entered: Semaphore,
    release: Semaphore,
}

#[async_trait]
impl ControlStore for PausingStore {
    async fn load(&self) -> Result<ait_ports::ControlSnapshot, ait_ports::ControlStoreError> {
        self.inner.load().await
    }
    async fn replay(
        &self,
        after: u64,
        limit: usize,
    ) -> Result<Vec<ait_ports::DurableEvent>, ait_ports::ControlStoreError> {
        self.inner.replay(after, limit).await
    }
    async fn commit(
        &self,
        revision: u64,
        value: serde_json::Value,
        events: Vec<ait_ports::PendingEvent>,
    ) -> Result<ait_ports::ControlSnapshot, ait_ports::ControlStoreError> {
        if value["runs"]
            .as_array()
            .is_some_and(|runs| !runs.is_empty())
        {
            self.entered.add_permits(1);
            self.release.acquire().await.unwrap().forget();
        }
        self.inner.commit(revision, value, events).await
    }
}

#[tokio::test]
async fn pessimistic_admission_rejects_a_competing_send_before_the_first_run_is_committed() {
    let store = Arc::new(PausingStore {
        inner: SqliteControlStore::in_memory().unwrap(),
        entered: Semaphore::new(0),
        release: Semaphore::new(0),
    });
    let service = Arc::new(LocalControlService::new(store.clone()));
    let _directory = setup(
        &service,
        AgentConfiguration {
            provider_id: "builtin-manual".into(),
            model: "default".into(),
            reasoning_effort: None,
        },
    )
    .await;
    let first = {
        let service = service.clone();
        tokio::spawn(async move { ok(&service, send("one")).await })
    };
    tokio::time::timeout(Duration::from_secs(3), store.entered.acquire())
        .await
        .unwrap()
        .unwrap()
        .forget();
    assert!(view(&service).await.runs.is_empty());
    let rejected = tokio::time::timeout(Duration::from_millis(500), service.execute(send("one")))
        .await
        .unwrap();
    assert_eq!(rejected.error.unwrap().code, ErrorCode::SessionBusy);
    assert_eq!(view(&service).await.messages.len(), 1);
    store.release.add_permits(1);
    first.await.unwrap();
    assert_eq!(view(&service).await.runs.len(), 1);
}
