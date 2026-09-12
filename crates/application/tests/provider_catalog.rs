//! Provider catalog regression coverage.
#![allow(clippy::pedantic)]

mod fixtures;
mod support;

use crate::fixtures::control_fixtures::{config, ok, send, setup, view};
use crate::fixtures::provider_fixtures::Gateway;
#[cfg(not(all(feature = "dev-mock-provider", debug_assertions)))]
use crate::fixtures::provider_fixtures::RETIRED_BUILTINS;
use crate::fixtures::workspace_agents::CapturingWorkspaceAgent;
use crate::support::ControlStoreTestExt;
use ait_application::LocalControlService;
use ait_contracts::{
    AgentConfiguration, AgentMode, AgentProvider, Command, CommandResult, ProviderModel,
    ProviderSecret,
};
use ait_domain::{DomainError, ErrorCode};
use ait_ports::HostProviderModelCatalog;
use ait_storage_sqlite::SqliteControlStore;
use async_trait::async_trait;
use std::sync::{Arc, Mutex};

#[cfg(all(feature = "dev-mock-provider", debug_assertions))]
fn mock_config() -> AgentConfiguration {
    AgentConfiguration {
        provider_id: "builtin-mock".into(),
        model: "mock-local".into(),
        reasoning_effort: None,
    }
}

#[derive(Default)]
struct HostCatalog(Mutex<Vec<AgentProvider>>);

#[async_trait]
impl HostProviderModelCatalog for HostCatalog {
    async fn discover_models(
        &self,
        provider: &AgentProvider,
    ) -> Result<Vec<ProviderModel>, DomainError> {
        self.0.lock().unwrap().push(provider.clone());
        Ok(vec![
            ProviderModel {
                id: "gpt-new".into(),
                name: "GPT New".into(),
                reasoning_efforts: vec!["low".into(), "high".into()],
            },
            ProviderModel {
                id: "gpt-fast".into(),
                name: "GPT Fast".into(),
                reasoning_efforts: vec!["medium".into()],
            },
        ])
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
                Some("AIT project instructions")
            );
            assert!(!native_calls[0].prompt.contains("AIT project instructions"));
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
            view(&service)
                .await
                .messages
                .into_iter()
                .find(|message| message.role == "system")
                .unwrap()
                .text
                .as_deref(),
            Some("AIT project instructions")
        );
    }
}

#[tokio::test]
async fn codex_discovery_uses_the_host_catalog_without_persisting_results() {
    let store = Arc::new(SqliteControlStore::in_memory().unwrap());
    let catalog = Arc::new(HostCatalog::default());
    let service =
        LocalControlService::new(store.clone()).with_host_provider_catalog(catalog.clone());
    let provider = view(&service)
        .await
        .providers
        .into_iter()
        .find(|view| view.provider.kind == AgentMode::Codex)
        .unwrap()
        .provider;
    let before = store.load().await.unwrap();
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
    assert_eq!(
        models
            .iter()
            .map(|model| model.id.as_str())
            .collect::<Vec<_>>(),
        ["gpt-new", "gpt-fast"]
    );
    assert_eq!(models[0].reasoning_efforts, ["low", "high"]);
    assert_eq!(
        catalog.0.lock().unwrap().as_slice(),
        std::slice::from_ref(&provider)
    );
    let after = store.load().await.unwrap();
    assert_eq!(after.revision, before.revision);
    assert_eq!(after.value, before.value);

    let rejected = service
        .execute(Command::DiscoverProviderModels {
            provider,
            secret: Some(ProviderSecret("must-not-be-used".into())),
        })
        .await;
    assert_eq!(
        rejected.error.unwrap().code,
        ErrorCode::InvalidAgentConfiguration
    );
    assert_eq!(catalog.0.lock().unwrap().len(), 1);
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

    // Once an adapter advertises capabilities, those facts supersede an older
    // empty/manual catalog. Preview remains side-effect-free; refresh persists
    // the new capabilities for Agent validation and desktop projection.
    let advertised = ["off", "low", "high", "max"].map(str::to_owned).to_vec();
    gateway
        .reasoning_efforts
        .lock()
        .unwrap()
        .clone_from(&advertised);
    let before = store.load().await.unwrap();
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
    assert!(
        models
            .iter()
            .all(|model| model.reasoning_efforts == advertised)
    );
    assert_eq!(store.load().await.unwrap(), before);

    let CommandResult::AgentProvider(refreshed) = ok(
        &service,
        Command::RefreshProviderModels {
            provider_id: provider.id.clone(),
        },
    )
    .await
    else {
        panic!()
    };
    assert!(
        refreshed
            .provider
            .models
            .iter()
            .all(|model| model.reasoning_efforts == advertised)
    );
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

#[cfg(not(all(feature = "dev-mock-provider", debug_assertions)))]
#[tokio::test]
async fn fresh_workspace_exposes_only_the_codex_builtin() {
    let service = LocalControlService::new(Arc::new(SqliteControlStore::in_memory().unwrap()));
    let providers = view(&service).await.providers;
    assert_eq!(providers.len(), 1);
    assert_eq!(providers[0].provider.id, "builtin-codex");
    assert_eq!(providers[0].provider.kind, AgentMode::Codex);
    assert!(serde_json::from_value::<AgentMode>(serde_json::json!("mock")).is_err());
    for kind in RETIRED_BUILTINS {
        assert!(serde_json::from_value::<AgentMode>(serde_json::json!(kind)).is_err());
    }
}

#[cfg(all(feature = "dev-mock-provider", debug_assertions))]
#[tokio::test]
async fn development_mock_is_selectable_and_persists_without_external_executors() {
    let database_directory = tempfile::tempdir().unwrap();
    let database = database_directory.path().join("mock-control.sqlite3");
    let store = Arc::new(ait_storage_sqlite::SplitSqliteControlStore::open(&database).unwrap());
    // No Provider gateway or Codex workspace harness is installed. A completed
    // result therefore proves the Mock invocation stayed on its local branch.
    let service = LocalControlService::new(store.clone());
    let providers = view(&service).await.providers;
    assert_eq!(providers.len(), 2);
    let mock = providers
        .iter()
        .find(|provider| provider.provider.id == "builtin-mock")
        .expect("development Mock provider");
    assert_eq!(mock.provider.kind, AgentMode::Mock);
    assert_eq!(mock.provider.models[0].id, "mock-local");

    let _directory = setup(&service, mock_config()).await;
    let CommandResult::Run(run) = ok(
        &service,
        Command::SendMessage {
            session_id: "one".into(),
            text: "arbitrary user input".into(),
        },
    )
    .await
    else {
        panic!("expected Run")
    };
    assert_eq!(run.status, "completed");
    assert_eq!(run.provider.kind, AgentMode::Mock);
    ok(
        &service,
        Command::SetSessionTitle {
            session_id: "one".into(),
            title: "Arbitrary user input".into(),
        },
    )
    .await;
    let title = service
        .generate_session_title("one".into(), "arbitrary user input".into())
        .await;
    assert!(
        title.ok,
        "Mock title handling must not require a Codex title generator: {:?}",
        title.error
    );

    let custom = service
        .execute(Command::SaveAgentProvider {
            provider: AgentProvider {
                id: "custom-mock".into(),
                name: "Custom Mock".into(),
                kind: AgentMode::Mock,
                url: None,
                models: vec![ProviderModel {
                    id: "custom".into(),
                    name: "Custom".into(),
                    reasoning_efforts: Vec::new(),
                }],
            },
            secret: None,
        })
        .await;
    assert_eq!(
        custom.error.unwrap().code,
        ErrorCode::InvalidAgentConfiguration
    );
    let archive = service
        .execute(Command::ExportProject {
            project_id: "p".into(),
        })
        .await;
    assert_eq!(archive.error.unwrap().code, ErrorCode::InvalidProject);

    drop(service);
    drop(store);
    let reopened_store =
        Arc::new(ait_storage_sqlite::SplitSqliteControlStore::open(&database).unwrap());
    let reopened_service = LocalControlService::new(reopened_store);
    let persisted = view(&reopened_service).await;
    let user = persisted
        .messages
        .iter()
        .find(|message| message.id == run.base_message_id)
        .unwrap();
    let assistant = persisted
        .messages
        .iter()
        .find(|message| Some(message.id.as_str()) == run.last_message_id.as_deref())
        .unwrap();
    assert_eq!(user.role, "user");
    assert_eq!(user.text.as_deref(), Some("arbitrary user input"));
    assert_eq!(assistant.role, "assistant");
    assert_eq!(assistant.text.as_deref(), Some("Mock assistant response."));
    assert_eq!(
        assistant.parent_message_id.as_deref(),
        Some(user.id.as_str())
    );
    let restored_run = persisted
        .runs
        .iter()
        .find(|item| item.id == run.id)
        .unwrap();
    assert_eq!(restored_run.status, "completed");
    assert_eq!(
        restored_run.last_message_id.as_deref(),
        Some(assistant.id.as_str())
    );
    let restored_session = persisted
        .sessions
        .iter()
        .find(|item| item.id == "one")
        .unwrap();
    assert_eq!(restored_session.current_message_id, assistant.id);
    assert!(restored_session.active_run_id.is_none());
    assert_eq!(
        restored_session.title.as_deref(),
        Some("Arbitrary user input")
    );
    assert!(restored_session.title_generation_started);
}
