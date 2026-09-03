//! End-to-end control-plane acceptance coverage.
#![allow(clippy::pedantic)]

use std::sync::Arc;

use ait_application::LocalControlService;
use ait_contracts::{AgentMode, Command, CommandResult};
use ait_domain::ErrorCode;
use ait_storage_sqlite::SqliteControlStore;
use tempfile::TempDir;

async fn run(service: &LocalControlService, command: Command) -> CommandResult {
    let response = service.execute(command).await;
    assert!(response.ok, "{:?}", response.error);
    response.result.unwrap()
}

#[tokio::test]
async fn tool_session_branch_cron_events_and_restart_form_one_vertical_slice() {
    let temporary = TempDir::new().unwrap();
    let database = temporary.path().join("ait.sqlite3");
    let project_dir = temporary.path().join("project");
    std::fs::create_dir(&project_dir).unwrap();
    let service = LocalControlService::new(Arc::new(SqliteControlStore::open(&database).unwrap()));

    let project = match run(
        &service,
        Command::RegisterProject {
            id: "project-1".into(),
            name: "Demo".into(),
            workdir: project_dir.display().to_string(),
        },
    )
    .await
    {
        CommandResult::Project(value) => value,
        _ => panic!(),
    };
    run(
        &service,
        Command::RegisterAgent {
            id: "agent-tool".into(),
            name: "Tool agent".into(),
            model: "deterministic-v1".into(),
            mode: AgentMode::Tool,
        },
    )
    .await;
    run(
        &service,
        Command::CreateSession {
            id: "session-main".into(),
            project_id: project.id.clone(),
            agent_id: "agent-tool".into(),
            at_message_id: None,
        },
    )
    .await;
    let interactive = match run(
        &service,
        Command::SendMessage {
            session_id: "session-main".into(),
            text: "use the echo tool".into(),
            expected_version: Some(1),
        },
    )
    .await
    {
        CommandResult::Run(value) => value,
        _ => panic!(),
    };
    assert_eq!(interactive.status, "completed");

    run(
        &service,
        Command::CreateSession {
            id: "session-branch".into(),
            project_id: project.id.clone(),
            agent_id: "agent-tool".into(),
            at_message_id: Some(project.root_message_id.clone()),
        },
    )
    .await;
    run(
        &service,
        Command::CreateCron {
            id: "cron-1".into(),
            name: "demo cron".into(),
            project_id: project.id,
            base_message_id: project.root_message_id,
            agent_id: "agent-tool".into(),
            schedule: "* * * * *".into(),
            timezone: "UTC".into(),
        },
    )
    .await;
    run(
        &service,
        Command::SetCronEnabled {
            cron_id: "cron-1".into(),
            enabled: false,
        },
    )
    .await;
    let disabled = service
        .execute(Command::TriggerCron {
            cron_id: "cron-1".into(),
            scheduled_at: 1_788_480_000_000,
        })
        .await;
    assert_eq!(disabled.error.unwrap().code, ErrorCode::InvalidCron);
    run(
        &service,
        Command::SetCronEnabled {
            cron_id: "cron-1".into(),
            enabled: true,
        },
    )
    .await;
    let scheduled = match run(
        &service,
        Command::TriggerCron {
            cron_id: "cron-1".into(),
            scheduled_at: 1_788_480_000_000,
        },
    )
    .await
    {
        CommandResult::Run(value) => value,
        _ => panic!(),
    };
    assert_eq!(scheduled.trigger, "cron");
    assert_eq!(scheduled.status, "completed");
    assert!(scheduled.session_id.is_none());

    let first_page = service.replay_events(0, 3).await.unwrap();
    assert_eq!(first_page.len(), 3);
    let remainder = service
        .replay_events(first_page.last().unwrap().cursor, 100)
        .await
        .unwrap();
    assert!(!remainder.is_empty());
    assert!(remainder[0].cursor > first_page.last().unwrap().cursor);

    drop(service);
    let recovered =
        LocalControlService::new(Arc::new(SqliteControlStore::open(&database).unwrap()));
    let workspace = match run(&recovered, Command::Snapshot).await {
        CommandResult::Workspace(value) => value,
        _ => panic!(),
    };
    assert_eq!(workspace.runs.len(), 2);
    assert_eq!(workspace.sessions.len(), 2);
    assert!(
        workspace
            .messages
            .iter()
            .any(|message| message.kind == "tool_result")
    );
    assert!(
        workspace
            .messages
            .iter()
            .filter(|message| message
                .data
                .as_ref()
                .is_some_and(|data| data.get("tool_use").is_some()))
            .count()
            >= 2
    );
}

#[tokio::test]
async fn stable_failures_cover_configuration_provider_approval_conflict_and_cancel() {
    let temporary = TempDir::new().unwrap();
    let project_dir = temporary.path().join("project");
    std::fs::create_dir(&project_dir).unwrap();
    let service = LocalControlService::new(Arc::new(SqliteControlStore::in_memory().unwrap()));
    let invalid = service
        .execute(Command::RegisterAgent {
            id: "bad".into(),
            name: "".into(),
            model: "m".into(),
            mode: AgentMode::Echo,
        })
        .await;
    assert_eq!(
        invalid.error.unwrap().code,
        ErrorCode::InvalidAgentConfiguration
    );
    let project = match run(
        &service,
        Command::RegisterProject {
            id: "p".into(),
            name: "P".into(),
            workdir: project_dir.display().to_string(),
        },
    )
    .await
    {
        CommandResult::Project(value) => value,
        _ => panic!(),
    };

    for (id, mode) in [
        ("provider", AgentMode::ProviderFailure),
        ("approval", AgentMode::ApprovalRequired),
        ("manual", AgentMode::Manual),
    ] {
        run(
            &service,
            Command::RegisterAgent {
                id: id.into(),
                name: id.into(),
                model: "m".into(),
                mode,
            },
        )
        .await;
        run(
            &service,
            Command::CreateSession {
                id: format!("s-{id}"),
                project_id: project.id.clone(),
                agent_id: id.into(),
                at_message_id: None,
            },
        )
        .await;
    }
    let provider = match run(
        &service,
        Command::SendMessage {
            session_id: "s-provider".into(),
            text: "go".into(),
            expected_version: Some(1),
        },
    )
    .await
    {
        CommandResult::Run(value) => value,
        _ => panic!(),
    };
    assert_eq!(provider.error.unwrap().code, ErrorCode::ProviderFailed);
    let approval = match run(
        &service,
        Command::SendMessage {
            session_id: "s-approval".into(),
            text: "go".into(),
            expected_version: Some(1),
        },
    )
    .await
    {
        CommandResult::Run(value) => value,
        _ => panic!(),
    };
    assert_eq!(
        approval.error.unwrap().code,
        ErrorCode::ToolApprovalRequired
    );

    let conflict = service
        .execute(Command::SendMessage {
            session_id: "s-manual".into(),
            text: "go".into(),
            expected_version: Some(99),
        })
        .await;
    assert_eq!(
        conflict.error.unwrap().code,
        ErrorCode::SessionPointerConflict
    );
    let queued = match run(
        &service,
        Command::SendMessage {
            session_id: "s-manual".into(),
            text: "go".into(),
            expected_version: Some(1),
        },
    )
    .await
    {
        CommandResult::Run(value) => value,
        _ => panic!(),
    };
    let cancelled = match run(&service, Command::CancelRun { run_id: queued.id }).await {
        CommandResult::Run(value) => value,
        _ => panic!(),
    };
    assert_eq!(cancelled.error.unwrap().code, ErrorCode::RunCancelled);
}
