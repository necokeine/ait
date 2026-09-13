//! Control fixtures regression coverage.
#![allow(clippy::pedantic)]
#![allow(dead_code)]
#![allow(missing_docs)]

use crate::support::{WorkspaceView, workspace};
use ait_application::LocalControlService;
use ait_contracts::{AgentConfiguration, Command, CommandResult, RunView, default_settings};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Semaphore;

pub(crate) fn config(effort: &str) -> AgentConfiguration {
    AgentConfiguration {
        provider_id: "builtin-codex".into(),
        model: "gpt-5.6-sol".into(),
        reasoning_effort: Some(effort.into()),
    }
}

pub(crate) fn send(id: &str) -> Command {
    Command::SendMessage {
        session_id: id.into(),
        text: "hello".into(),
    }
}

pub(crate) fn send_text(id: &str, text: &str) -> Command {
    Command::SendMessage {
        session_id: id.into(),
        text: text.into(),
    }
}

pub(crate) async fn ok(service: &LocalControlService, command: Command) -> CommandResult {
    let response = service.execute(command).await;
    assert!(response.ok, "{:?}", response.error);
    response.result.unwrap()
}

pub(crate) async fn view(service: &LocalControlService) -> WorkspaceView {
    workspace(service).await
}

pub(crate) async fn submit_run(service: &Arc<LocalControlService>, command: Command) -> RunView {
    let response = service.submit(command).await;
    assert!(response.ok, "{:?}", response.error);
    let CommandResult::Run(run) = response.result.unwrap() else {
        panic!("expected Run")
    };
    run
}

pub(crate) async fn wait_for_signal(semaphore: &Semaphore) {
    tokio::time::timeout(Duration::from_secs(3), semaphore.acquire())
        .await
        .unwrap()
        .unwrap()
        .forget();
}

pub(crate) async fn setup(
    service: &LocalControlService,
    configuration: AgentConfiguration,
) -> tempfile::TempDir {
    let directory = tempfile::tempdir().unwrap();
    ok(
        service,
        Command::RegisterProject {
            id: "p".into(),
            name: "Project".into(),
            workdir: Some(directory.path().display().to_string()),
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

pub(crate) fn git_head(path: &std::path::Path) -> String {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(path)
        .args(["rev-parse", "HEAD"])
        .output()
        .unwrap();
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

pub(crate) fn git_index_tree(path: &std::path::Path) -> String {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(path)
        .arg("write-tree")
        .output()
        .unwrap();
    assert!(output.status.success());
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

pub(crate) async fn save_permission_settings(
    service: &LocalControlService,
    sandbox: &str,
    approval: &str,
) {
    let mut values = default_settings();
    values
        .0
        .insert("permissions.sandbox".into(), serde_json::json!(sandbox));
    values
        .0
        .insert("permissions.approval".into(), serde_json::json!(approval));
    let _ = ok(
        service,
        Command::SaveSettings {
            expected_revision: 1,
            values,
        },
    )
    .await;
}
