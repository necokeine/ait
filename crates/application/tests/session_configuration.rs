//! Session configuration regression coverage.
#![allow(clippy::pedantic)]

use ait_ports::ControlStore;
mod fixtures;
mod support;

use crate::fixtures::control_fixtures::{config, ok, send, setup, view};
use crate::fixtures::workspace_agents::BlockingAgent;
use crate::support::ControlStoreTestExt;
use ait_application::LocalControlService;
use ait_contracts::{Command, CommandResult, default_settings};
use ait_domain::ErrorCode;
use ait_ports::{ControlChange, ControlFilter, ControlRecord, ControlRecordKind};
use ait_storage_sqlite::SqliteControlStore;
use std::sync::Arc;
use std::time::Duration;

#[tokio::test]
async fn session_config_is_private_reused_and_copied_when_opening_another_session() {
    let store = Arc::new(SqliteControlStore::in_memory().unwrap());
    let service = LocalControlService::new(
        std::sync::Arc::new(ait_workspace_local::LocalProjectWorkspace::default()),
        store.clone(),
    );
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
    let rejected_edit = service
        .execute(Command::UpdateProject {
            project_id: "p".into(),
            name: "Must not change".into(),
            agent_id: Some(custom.clone()),
        })
        .await;
    assert_eq!(
        rejected_edit.error.unwrap().code,
        ErrorCode::InvalidAgentConfiguration
    );
    assert_eq!(view(&service).await, after);
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
    let restarted = LocalControlService::new(
        std::sync::Arc::new(ait_workspace_local::LocalProjectWorkspace::default()),
        store,
    );
    assert_eq!(view(&restarted).await, after);
}

#[tokio::test]
async fn cancelling_an_active_call_releases_session_and_retains_confirmed_native_input() {
    let store = Arc::new(SqliteControlStore::in_memory().unwrap());
    let agent = Arc::new(BlockingAgent::new());
    let service = Arc::new(crate::support::native::native_service(
        std::sync::Arc::new(ait_workspace_local::LocalProjectWorkspace::default()),
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
    assert_eq!(view(&service).await.messages.len(), 3);
    assert!(cancelled.git_commit.is_none());
    agent.release.add_permits(1);
    let CommandResult::Run(next) = ok(&service, send("one")).await else {
        panic!()
    };
    assert_eq!(next.status, "completed");
}

#[tokio::test]
async fn unrelated_malformed_project_record_does_not_block_session_update() {
    let store = Arc::new(SqliteControlStore::in_memory().unwrap());
    let service = LocalControlService::new(
        std::sync::Arc::new(ait_workspace_local::LocalProjectWorkspace::default()),
        store.clone(),
    );
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

#[tokio::test]
async fn project_edits_are_atomic_persisted_and_preserve_existing_sessions_and_provenance() {
    let store = Arc::new(SqliteControlStore::in_memory().unwrap());
    let service = LocalControlService::new(
        Arc::new(ait_workspace_local::LocalProjectWorkspace::default()),
        store.clone(),
    );
    let _directory = setup(&service, config("high")).await;
    let before = view(&service).await;
    ok(
        &service,
        Command::RegisterAgent {
            id: "alternate".into(),
            name: "Alternate".into(),
            config: config("low"),
        },
    )
    .await;
    let result = ok(
        &service,
        Command::UpdateProject {
            project_id: "p".into(),
            name: "  中文 renamed Project  ".into(),
            agent_id: Some("alternate".into()),
        },
    )
    .await;
    let CommandResult::Project(updated) = result else {
        panic!("Project result")
    };
    assert_eq!(updated.name, "中文 renamed Project");
    assert_eq!(updated.default_agent_id.as_deref(), Some("alternate"));
    assert_eq!(updated.revision, before.projects[0].revision + 1);
    assert_eq!(updated.workdir, before.projects[0].workdir);
    assert_eq!(updated.base_commit, before.projects[0].base_commit);
    assert_eq!(updated.repo_url, before.projects[0].repo_url);
    assert_eq!(updated.root_message_id, before.projects[0].root_message_id);
    let after = view(&service).await;
    assert_eq!(after.sessions, before.sessions);
    assert_eq!(after.messages, before.messages);
    for (project_id, name, agent_id, code) in [
        ("p", " ", Some("preset"), ErrorCode::InvalidProject),
        (
            "p",
            "Must not be saved",
            Some("missing"),
            ErrorCode::AgentNotFound,
        ),
        (
            "missing",
            "Unknown Project",
            Some("preset"),
            ErrorCode::InvalidProject,
        ),
    ] {
        let response = service
            .execute(Command::UpdateProject {
                project_id: project_id.into(),
                name: name.into(),
                agent_id: agent_id.map(str::to_owned),
            })
            .await;
        assert_eq!(response.error.unwrap().code, code);
        assert_eq!(view(&service).await, after);
    }
    ok(
        &service,
        Command::UpdateProject {
            project_id: "p".into(),
            name: "Name only".into(),
            agent_id: None,
        },
    )
    .await;
    let after = view(&service).await;
    assert_eq!(after.projects[0].name, "Name only");
    assert_eq!(
        after.projects[0].default_agent_id.as_deref(),
        Some("alternate")
    );
    assert_eq!(after.projects[0].revision, updated.revision + 1);
    let mut invalid_settings = default_settings();
    invalid_settings
        .0
        .insert("agents.default_agent".into(), serde_json::json!("missing"));
    let rejected = service
        .execute(Command::SaveSettings {
            expected_revision: 1,
            values: invalid_settings,
        })
        .await;
    assert_eq!(rejected.error.unwrap().code, ErrorCode::AgentNotFound);

    let mut settings = default_settings();
    settings
        .0
        .insert("agents.default_agent".into(), serde_json::json!("preset"));
    ok(
        &service,
        Command::SaveSettings {
            expected_revision: 1,
            values: settings,
        },
    )
    .await;
    let cleared = ok(
        &service,
        Command::UpdateProject {
            project_id: "p".into(),
            name: "Global default".into(),
            agent_id: Some(String::new()),
        },
    )
    .await;
    let CommandResult::Project(cleared) = cleared else {
        panic!("Project result")
    };
    assert_eq!(cleared.default_agent_id, None);
    let created = ok(
        &service,
        Command::CreateSession {
            id: "global-default".into(),
            project_id: "p".into(),
            agent_id: String::new(),
            at_message_id: None,
        },
    )
    .await;
    let CommandResult::Session(created) = created else {
        panic!("Session result")
    };
    assert_eq!(created.agent_id, "preset");
    let cron = ok(
        &service,
        Command::CreateCron {
            id: "global-default-cron".into(),
            name: "Global default".into(),
            project_id: "p".into(),
            base_message_id: after.projects[0].root_message_id.clone(),
            agent_id: String::new(),
            schedule: "* * * * *".into(),
            timezone: "UTC".into(),
        },
    )
    .await;
    let CommandResult::Cron(cron) = cron else {
        panic!("Cron result")
    };
    assert_eq!(cron.agent_id, "preset");
    let after = view(&service).await;
    let restarted = LocalControlService::new(
        Arc::new(ait_workspace_local::LocalProjectWorkspace::default()),
        store,
    );
    assert_eq!(view(&restarted).await, after);
}
