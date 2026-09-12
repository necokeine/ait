//! Session configuration regression coverage.
#![allow(clippy::pedantic)]

use ait_ports::ControlStore;
mod fixtures;
mod support;

use crate::fixtures::control_fixtures::{config, ok, send, setup, view};
use crate::fixtures::workspace_agents::BlockingAgent;
use crate::support::ControlStoreTestExt;
use ait_application::LocalControlService;
use ait_contracts::{Command, CommandResult};
use ait_domain::ErrorCode;
use ait_ports::{ControlChange, ControlFilter, ControlRecord, ControlRecordKind};
use ait_storage_sqlite::SqliteControlStore;
use std::sync::Arc;
use std::time::Duration;

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
    let first_session = first
        .sessions
        .iter()
        .find(|session| session.id == "one")
        .unwrap();
    let custom = first_session.agent_id.clone();
    assert_ne!(custom, "preset");
    assert_eq!(
        first
            .sessions
            .iter()
            .find(|session| session.id == "two")
            .unwrap()
            .agent_id,
        "preset"
    );
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
            at_message_id: Some(first_session.current_message_id.clone()),
        },
    )
    .await;
    let after = view(&service).await;
    assert_eq!(
        after
            .sessions
            .iter()
            .find(|session| session.id == "one")
            .unwrap()
            .agent_id,
        custom
    );
    assert_ne!(
        after
            .sessions
            .iter()
            .find(|session| session.id == "branch")
            .unwrap()
            .agent_id,
        custom
    );
    let saved = after
        .agents
        .iter()
        .find(|agent| agent.id == custom)
        .unwrap();
    assert!(saved.name.is_empty());
    assert_eq!(saved.revision, 2);
    assert_eq!(saved.config, config("medium"));
    assert_eq!(
        after
            .agents
            .iter()
            .find(|agent| agent.id == "preset")
            .unwrap()
            .config,
        config("high")
    );
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

#[tokio::test]
async fn unrelated_malformed_project_record_does_not_block_session_update() {
    let store = Arc::new(SqliteControlStore::in_memory().unwrap());
    let service = LocalControlService::new(store.clone());
    let _directory = setup(&service, config("high")).await;
    let revision = store.load().await.unwrap().revision;
    store
        .apply(
            revision,
            vec![ControlChange::Put(ControlRecord {
                kind: ControlRecordKind::Message,
                id: "malformed-unrelated-message".into(),
                project_id: Some("p".into()),
                value: serde_json::json!({"not": "a MessageView"}),
            })],
            Vec::new(),
        )
        .await
        .unwrap();

    let CommandResult::Session(session) = ok(
        &service,
        Command::RenameSession {
            session_id: "one".into(),
            name: "Still available".into(),
        },
    )
    .await
    else {
        panic!()
    };
    assert_eq!(session.name, "Still available");
    let malformed = store
        .read(&[ControlFilter::id(
            ControlRecordKind::Message,
            "malformed-unrelated-message",
        )])
        .await
        .unwrap();
    assert_eq!(malformed.records[0].value["not"], "a MessageView");
}
