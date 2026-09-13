//! Provider migration regression coverage.
#![allow(clippy::pedantic)]

mod fixtures;
mod support;

use crate::fixtures::control_fixtures::{config, ok, send, setup, view};
use crate::fixtures::provider_fixtures::RETIRED_BUILTINS;
use crate::support::ControlStoreTestExt;
use ait_application::LocalControlService;
use ait_contracts::{Command, CommandResult};
use ait_domain::ErrorCode;
use ait_storage_sqlite::SqliteControlStore;
use std::sync::Arc;

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
        .extend(RETIRED_BUILTINS.map(retired_builtin));
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
    // An unrelated Session write does not rewrite or compact the provider table.
    assert_eq!(saved.value["providers"].as_array().unwrap().len(), 4);
    assert_eq!(view(&reopened).await.providers, before.providers);
    assert_eq!(
        view(&LocalControlService::new(store)).await.messages,
        before.messages
    );
}

#[tokio::test]
async fn retired_provider_references_and_custom_connections_are_never_silently_removed() {
    for kind in RETIRED_BUILTINS {
        for reference in ["agent", "run", "credential", "custom", "url"] {
            let store = Arc::new(SqliteControlStore::in_memory().unwrap());
            let service = LocalControlService::new(store.clone());
            let _directory = setup(&service, config("high")).await;
            ok(&service, send("one")).await;
            let mut snapshot = store.load().await.unwrap();
            let id = format!("builtin-{kind}");
            let mut retired = retired_builtin(kind);
            match reference {
                "agent" => {
                    snapshot.value["agents"][0]["config"]["provider_id"] = serde_json::json!(id)
                }
                "run" => snapshot.value["runs"][0]["provider"] = retired_provider(kind),
                "credential" => {
                    snapshot.value["provider_credentials"][&id] =
                        serde_json::json!("opaque-reference")
                }
                "custom" => retired["id"] = serde_json::json!(format!("custom-{kind}")),
                "url" => retired["url"] = serde_json::json!("http://localhost:1234"),
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
            let command = match reference {
                "agent" => send("one"),
                "run" => Command::GetRun {
                    run_id: saved.value["runs"][0]["id"].as_str().unwrap().into(),
                },
                "credential" | "custom" | "url" => Command::ListAgentProviders,
                _ => unreachable!(),
            };
            let rejected = LocalControlService::new(store.clone())
                .execute(command)
                .await;
            assert!(!rejected.ok, "{kind}/{reference} unexpectedly decoded");
            assert_eq!(rejected.error.unwrap().code, ErrorCode::RunRecoveryFailed);
            assert_eq!(store.load().await.unwrap().value, saved.value);
        }
    }
}

fn retired_builtin(kind: &str) -> serde_json::Value {
    let mut provider = retired_provider(kind);
    provider["has_secret"] = serde_json::json!(false);
    provider
}

fn retired_provider(kind: &str) -> serde_json::Value {
    serde_json::json!({
        "id": format!("builtin-{kind}"), "name": kind, "kind": kind,
        "url": null, "models": [{"id": "default", "name": "Default", "reasoning_efforts": []}],
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
