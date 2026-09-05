//! End-to-end control-plane acceptance coverage.
#![allow(clippy::pedantic)]

use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use ait_application::LocalControlService;
use ait_contracts::{AgentMode, Command, CommandResult, ReasoningEffort, default_settings};
use ait_domain::{DomainError, ErrorCode};
use ait_ports::{
    GeneratedSessionTitle, SessionTitleGenerator, SessionTitleRequest, WorkspaceAgent,
    WorkspaceAgentInvocation, WorkspaceAgentResponse,
};
use ait_storage_sqlite::SqliteControlStore;
use async_trait::async_trait;
use tempfile::TempDir;

async fn run(service: &LocalControlService, command: Command) -> CommandResult {
    let response = service.execute(command).await;
    assert!(response.ok, "{:?}", response.error);
    response.result.unwrap()
}

#[tokio::test]
async fn user_message_requires_clean_git_and_records_head_commit() {
    let temporary = TempDir::new().unwrap();
    let project_dir = temporary.path().join("project");
    std::fs::create_dir(&project_dir).unwrap();
    let service = LocalControlService::new(Arc::new(SqliteControlStore::in_memory().unwrap()));
    let project = match run(
        &service,
        Command::RegisterProject {
            id: "git-project".into(),
            name: "Git Project".into(),
            workdir: project_dir.display().to_string(),
            fork_repo_url: Some("git@github.com:member/fork.git".into()),
        },
    )
    .await
    {
        CommandResult::Project(project) => project,
        _ => panic!(),
    };
    assert_eq!(project.base_commit.len(), 40);
    assert_eq!(
        project.fork_repo_url.as_deref(),
        Some("git@github.com:member/fork.git")
    );
    run(
        &service,
        Command::RegisterAgent {
            id: "echo".into(),
            name: "Echo".into(),
            model: "echo".into(),
            mode: AgentMode::Echo,
        },
    )
    .await;
    run(
        &service,
        Command::CreateSession {
            id: "git-session".into(),
            project_id: project.id.clone(),
            agent_id: "echo".into(),
            at_message_id: None,
        },
    )
    .await;

    let dirty_path = project_dir.join("dirty.txt");
    std::fs::write(&dirty_path, "dirty").unwrap();
    let rejected = service
        .execute(Command::SendMessage {
            session_id: "git-session".into(),
            text: "must not append".into(),
            expected_version: Some(1),
            reasoning_effort: None,
        })
        .await;
    assert_eq!(rejected.error.unwrap().code, ErrorCode::ProjectGitDirty);
    std::fs::remove_file(dirty_path).unwrap();

    run(
        &service,
        Command::SendMessage {
            session_id: "git-session".into(),
            text: "append clean input".into(),
            expected_version: Some(1),
            reasoning_effort: None,
        },
    )
    .await;
    let workspace = match run(&service, Command::Snapshot).await {
        CommandResult::Workspace(workspace) => workspace,
        _ => panic!(),
    };
    let user = workspace
        .messages
        .iter()
        .find(|message| message.text.as_deref() == Some("append clean input"))
        .unwrap();
    assert_eq!(
        user.git_commit.as_deref(),
        Some(project.base_commit.as_str())
    );
}

#[derive(Debug)]
struct SuccessfulCodex;

#[async_trait]
impl WorkspaceAgent for SuccessfulCodex {
    async fn invoke(
        &self,
        request: WorkspaceAgentInvocation,
    ) -> Result<WorkspaceAgentResponse, DomainError> {
        assert!(request.prompt.contains("user: implement the feature"));
        assert_eq!(request.commit_subject, "implement the feature");
        assert_eq!(request.reasoning_effort.as_deref(), Some("high"));
        Ok(WorkspaceAgentResponse {
            assistant_text: "Implemented and verified the feature.".into(),
            commit_id: Some("0123456789abcdef".into()),
        })
    }
}

#[derive(Debug)]
struct SuccessfulTitleGenerator {
    calls: Arc<AtomicUsize>,
}

#[async_trait]
impl SessionTitleGenerator for SuccessfulTitleGenerator {
    async fn generate(
        &self,
        request: SessionTitleRequest,
    ) -> Result<GeneratedSessionTitle, DomainError> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        assert_eq!(request.user_prompt.chars().count(), 2_000);
        Ok(GeneratedSessionTitle {
            title: "Implement session naming".into(),
            description: "Add editable and generated Session names".into(),
        })
    }
}

#[tokio::test]
async fn first_interaction_generates_session_metadata_once_and_preserves_manual_name() {
    let temporary = TempDir::new().unwrap();
    let project_dir = temporary.path().join("project");
    std::fs::create_dir(&project_dir).unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let service = LocalControlService::new(Arc::new(SqliteControlStore::in_memory().unwrap()))
        .with_session_title_generator(Arc::new(SuccessfulTitleGenerator {
            calls: calls.clone(),
        }));
    let project = match run(
        &service,
        Command::RegisterProject {
            id: "named-project".into(),
            name: "Named Project".into(),
            workdir: project_dir.display().to_string(),
            fork_repo_url: None,
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
            id: "echo-agent".into(),
            name: "Echo".into(),
            model: "echo".into(),
            mode: AgentMode::Echo,
        },
    )
    .await;
    run(
        &service,
        Command::CreateSession {
            id: "named-session".into(),
            project_id: project.id,
            agent_id: "echo-agent".into(),
            at_message_id: None,
        },
    )
    .await;
    run(
        &service,
        Command::SetSessionTitle {
            session_id: "named-session".into(),
            title: "Temporary prompt title".into(),
        },
    )
    .await;
    run(
        &service,
        Command::SendMessage {
            session_id: "named-session".into(),
            text: "first interaction".into(),
            expected_version: Some(1),
            reasoning_effort: None,
        },
    )
    .await;
    let pointer_version = match run(&service, Command::Snapshot).await {
        CommandResult::Workspace(value) => value.sessions[0].version,
        _ => panic!(),
    };

    let first = service
        .generate_session_title("named-session".into(), "x".repeat(2_100))
        .await;
    assert!(first.ok, "{:?}", first.error);
    let second = service
        .generate_session_title("named-session".into(), "x".repeat(2_100))
        .await;
    assert!(second.ok, "{:?}", second.error);
    assert_eq!(calls.load(Ordering::Relaxed), 1);

    let renamed = match run(
        &service,
        Command::RenameSession {
            session_id: "named-session".into(),
            name: "  My   Session  ".into(),
        },
    )
    .await
    {
        CommandResult::Session(value) => value,
        _ => panic!(),
    };
    assert_eq!(renamed.name, "My Session");
    assert_eq!(renamed.title.as_deref(), Some("Implement session naming"));
    assert_eq!(
        renamed.description,
        "Add editable and generated Session names"
    );
    assert!(renamed.title_generation_started);
    assert_eq!(
        renamed.version, pointer_version,
        "metadata updates must not move the pointer version"
    );
}

#[tokio::test]
async fn codex_session_persists_assistant_result_and_commit_reference() {
    let temporary = TempDir::new().unwrap();
    let project_dir = temporary.path().join("project");
    std::fs::create_dir(&project_dir).unwrap();
    let service = LocalControlService::with_workspace_agent(
        Arc::new(SqliteControlStore::in_memory().unwrap()),
        Arc::new(SuccessfulCodex),
    );
    let project = match run(
        &service,
        Command::RegisterProject {
            id: "codex-project".into(),
            name: "Codex Project".into(),
            workdir: project_dir.display().to_string(),
            fork_repo_url: None,
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
            id: "codex-agent".into(),
            name: "Codex".into(),
            model: "gpt-5.6-sol".into(),
            mode: AgentMode::Codex,
        },
    )
    .await;
    run(
        &service,
        Command::CreateSession {
            id: "codex-session".into(),
            project_id: project.id,
            agent_id: "codex-agent".into(),
            at_message_id: None,
        },
    )
    .await;
    let completed = match run(
        &service,
        Command::SendMessage {
            session_id: "codex-session".into(),
            text: "implement the feature".into(),
            expected_version: Some(1),
            reasoning_effort: Some(ReasoningEffort::High),
        },
    )
    .await
    {
        CommandResult::Run(value) => value,
        _ => panic!(),
    };
    assert_eq!(completed.status, "completed");
    assert_eq!(completed.reasoning_effort, Some(ReasoningEffort::High));

    let workspace = match run(&service, Command::Snapshot).await {
        CommandResult::Workspace(value) => value,
        _ => panic!(),
    };
    let session = &workspace.sessions[0];
    assert!(session.active_run_id.is_none());
    let assistant = workspace
        .messages
        .iter()
        .find(|message| message.id == session.current_message_id)
        .unwrap();
    assert_eq!(assistant.role, "assistant");
    assert_eq!(
        assistant.text.as_deref(),
        Some("Implemented and verified the feature.")
    );
    assert_eq!(
        assistant.data.as_ref().unwrap()["codex"]["commit_id"],
        serde_json::json!("0123456789abcdef")
    );
}

#[tokio::test]
async fn idle_session_can_rebind_agent_with_version_cas() {
    let temporary = TempDir::new().unwrap();
    let project_dir = temporary.path().join("project");
    std::fs::create_dir(&project_dir).unwrap();
    let service = LocalControlService::new(Arc::new(SqliteControlStore::in_memory().unwrap()));
    let project = match run(
        &service,
        Command::RegisterProject {
            id: "rebind-project".into(),
            name: "Rebind Project".into(),
            workdir: project_dir.display().to_string(),
            fork_repo_url: None,
        },
    )
    .await
    {
        CommandResult::Project(value) => value,
        _ => panic!(),
    };
    for id in ["echo-agent", "manual-agent"] {
        run(
            &service,
            Command::RegisterAgent {
                id: id.into(),
                name: id.into(),
                model: id.into(),
                mode: if id == "echo-agent" {
                    AgentMode::Echo
                } else {
                    AgentMode::Manual
                },
            },
        )
        .await;
    }
    run(
        &service,
        Command::CreateSession {
            id: "rebind-session".into(),
            project_id: project.id,
            agent_id: "echo-agent".into(),
            at_message_id: None,
        },
    )
    .await;
    let rebound = match run(
        &service,
        Command::SetSessionAgent {
            session_id: "rebind-session".into(),
            agent_id: "manual-agent".into(),
            expected_version: Some(1),
        },
    )
    .await
    {
        CommandResult::Session(value) => value,
        _ => panic!(),
    };
    assert_eq!(rebound.agent_id, "manual-agent");
    assert_eq!(rebound.version, 2);

    let stale = service
        .execute(Command::SetSessionAgent {
            session_id: "rebind-session".into(),
            agent_id: "echo-agent".into(),
            expected_version: Some(1),
        })
        .await;
    assert_eq!(stale.error.unwrap().code, ErrorCode::SessionPointerConflict);
    let queued = match run(
        &service,
        Command::SendMessage {
            session_id: "rebind-session".into(),
            text: "keep this run queued".into(),
            expected_version: Some(2),
            reasoning_effort: None,
        },
    )
    .await
    {
        CommandResult::Run(value) => value,
        _ => panic!(),
    };
    assert_eq!(queued.agent_id, "manual-agent");
    let busy = service
        .execute(Command::SetSessionAgent {
            session_id: "rebind-session".into(),
            agent_id: "echo-agent".into(),
            expected_version: Some(3),
        })
        .await;
    assert_eq!(busy.error.unwrap().code, ErrorCode::SessionBusy);
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
            fork_repo_url: None,
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
            reasoning_effort: None,
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
            fork_repo_url: None,
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
            reasoning_effort: None,
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
            reasoning_effort: None,
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

    let unsupported_effort = service
        .execute(Command::SendMessage {
            session_id: "s-manual".into(),
            text: "go".into(),
            expected_version: Some(1),
            reasoning_effort: Some(ReasoningEffort::High),
        })
        .await;
    assert_eq!(
        unsupported_effort.error.unwrap().code,
        ErrorCode::InvalidAgentConfiguration
    );

    let conflict = service
        .execute(Command::SendMessage {
            session_id: "s-manual".into(),
            text: "go".into(),
            expected_version: Some(99),
            reasoning_effort: None,
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
            reasoning_effort: None,
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

#[tokio::test]
async fn project_export_import_preserves_tree_and_revisions_without_runtime_or_credentials() {
    let temporary = TempDir::new().unwrap();
    let source_dir = temporary.path().join("source");
    let imported_dir = temporary.path().join("imported");
    std::fs::create_dir(&source_dir).unwrap();
    std::fs::create_dir(&imported_dir).unwrap();
    let source = LocalControlService::new(Arc::new(SqliteControlStore::in_memory().unwrap()));
    let project = match run(
        &source,
        Command::RegisterProject {
            id: "portable-project".into(),
            name: "Portable".into(),
            workdir: source_dir.display().to_string(),
            fork_repo_url: None,
        },
    )
    .await
    {
        CommandResult::Project(value) => value,
        _ => panic!(),
    };
    run(
        &source,
        Command::RegisterAgent {
            id: "portable-agent".into(),
            name: "Portable agent".into(),
            model: "local-model".into(),
            mode: AgentMode::Manual,
        },
    )
    .await;
    run(
        &source,
        Command::SetProjectDefaultAgent {
            project_id: project.id.clone(),
            agent_id: "portable-agent".into(),
        },
    )
    .await;
    run(
        &source,
        Command::CreateSession {
            id: "portable-session".into(),
            project_id: project.id.clone(),
            agent_id: "portable-agent".into(),
            at_message_id: None,
        },
    )
    .await;
    run(
        &source,
        Command::SendMessage {
            session_id: "portable-session".into(),
            text: "preserve this branch".into(),
            expected_version: Some(1),
            reasoning_effort: None,
        },
    )
    .await;

    let archive = match run(
        &source,
        Command::ExportProject {
            project_id: project.id.clone(),
        },
    )
    .await
    {
        CommandResult::ProjectExport(value) => value,
        _ => panic!(),
    };
    let encoded = serde_json::to_string(&archive).unwrap();
    assert!(!encoded.contains("secret"));
    assert!(!encoded.contains("token"));
    assert!(!encoded.contains("credential"));
    assert_eq!(archive.project.revision, project.revision + 1);
    assert_eq!(
        archive.project.default_agent_id.as_deref(),
        Some("portable-agent")
    );
    assert_eq!(archive.sessions[0].version, 2);
    assert!(archive.sessions[0].active_run_id.is_none());
    assert_eq!(archive.messages.len(), 2);

    let target = LocalControlService::new(Arc::new(SqliteControlStore::in_memory().unwrap()));
    run(
        &target,
        Command::ImportProject {
            archive: archive.clone(),
            workdir: imported_dir.display().to_string(),
        },
    )
    .await;
    let workspace = match run(&target, Command::Snapshot).await {
        CommandResult::Workspace(value) => value,
        _ => panic!(),
    };
    assert_eq!(workspace.projects[0].revision, archive.project.revision);
    assert_eq!(
        workspace.projects[0].default_agent_id,
        archive.project.default_agent_id
    );
    assert_eq!(workspace.agents[0].revision, archive.agents[0].revision);
    assert_eq!(workspace.sessions[0].version, archive.sessions[0].version);
    assert_eq!(
        workspace.sessions[0].current_message_id,
        archive.sessions[0].current_message_id
    );
    assert_eq!(workspace.messages, archive.messages);
    assert!(workspace.runs.is_empty());
    assert!(workspace.crons.is_empty());
}

#[tokio::test]
async fn desktop_fork_and_settings_share_one_durable_daemon_state() {
    let temporary = TempDir::new().unwrap();
    let database = temporary.path().join("desktop.sqlite3");
    let project_dir = temporary.path().join("project");
    std::fs::create_dir(&project_dir).unwrap();
    let service = LocalControlService::new(Arc::new(SqliteControlStore::open(&database).unwrap()));
    let project = match run(
        &service,
        Command::RegisterProject {
            id: "desktop-project".into(),
            name: "Desktop".into(),
            workdir: project_dir.display().to_string(),
            fork_repo_url: None,
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
            id: "desktop-agent".into(),
            name: "Desktop agent".into(),
            model: "echo".into(),
            mode: AgentMode::Echo,
        },
    )
    .await;
    run(
        &service,
        Command::ForkSession {
            id: "desktop-branch".into(),
            project_id: project.id,
            agent_id: "desktop-agent".into(),
            at_message_id: project.root_message_id,
            text: "first branch message".into(),
            reasoning_effort: None,
        },
    )
    .await;

    let mut values = default_settings();
    values
        .0
        .insert("interface.theme".into(), serde_json::json!("dark"));
    let saved = match run(
        &service,
        Command::SaveSettings {
            expected_revision: 1,
            values,
        },
    )
    .await
    {
        CommandResult::Settings(value) => value,
        _ => panic!(),
    };
    assert_eq!(saved.revision, 2);
    drop(service);

    let recovered =
        LocalControlService::new(Arc::new(SqliteControlStore::open(&database).unwrap()));
    let workspace = match run(&recovered, Command::Snapshot).await {
        CommandResult::Workspace(value) => value,
        _ => panic!(),
    };
    assert_eq!(workspace.sessions.len(), 1);
    assert_eq!(workspace.messages.len(), 3);
    let settings = match run(&recovered, Command::GetSettings).await {
        CommandResult::Settings(value) => value,
        _ => panic!(),
    };
    assert_eq!(
        settings.values.0["interface.theme"],
        serde_json::json!("dark")
    );
}

#[tokio::test]
async fn desktop_two_project_flow_keeps_backends_sessions_and_replies_isolated() {
    let temporary = TempDir::new().unwrap();
    let project_a_dir = temporary.path().join("project-a");
    let project_b_dir = temporary.path().join("project-b");
    std::fs::create_dir(&project_a_dir).unwrap();
    std::fs::create_dir(&project_b_dir).unwrap();
    let service = LocalControlService::new(Arc::new(SqliteControlStore::in_memory().unwrap()));

    run(
        &service,
        Command::RegisterAgent {
            id: "codex-local".into(),
            name: "Codex".into(),
            model: "gpt-5.6-codex".into(),
            mode: AgentMode::Echo,
        },
    )
    .await;

    for (id, name, directory, session_id, input) in [
        (
            "project-a",
            "Project A",
            &project_a_dir,
            "session-a",
            "message for A",
        ),
        (
            "project-b",
            "Project B",
            &project_b_dir,
            "session-b",
            "message for B",
        ),
    ] {
        run(
            &service,
            Command::RegisterProject {
                id: id.into(),
                name: name.into(),
                workdir: directory.display().to_string(),
                fork_repo_url: None,
            },
        )
        .await;
        run(
            &service,
            Command::SetProjectDefaultAgent {
                project_id: id.into(),
                agent_id: "codex-local".into(),
            },
        )
        .await;
        run(
            &service,
            Command::CreateSession {
                id: session_id.into(),
                project_id: id.into(),
                agent_id: "codex-local".into(),
                at_message_id: None,
            },
        )
        .await;
        let completed = match run(
            &service,
            Command::SendMessage {
                session_id: session_id.into(),
                text: input.into(),
                expected_version: Some(1),
                reasoning_effort: None,
            },
        )
        .await
        {
            CommandResult::Run(value) => value,
            _ => panic!(),
        };
        assert_eq!(completed.status, "completed");
    }

    let workspace = match run(&service, Command::Snapshot).await {
        CommandResult::Workspace(value) => value,
        _ => panic!(),
    };
    assert_eq!(workspace.projects.len(), 2);
    assert!(workspace.projects.iter().all(|project| {
        project.default_agent_id.as_deref() == Some("codex-local") && project.revision == 2
    }));
    assert_eq!(workspace.sessions.len(), 2);
    for (project_id, session_id, input) in [
        ("project-a", "session-a", "message for A"),
        ("project-b", "session-b", "message for B"),
    ] {
        let session = workspace
            .sessions
            .iter()
            .find(|session| session.id == session_id)
            .unwrap();
        assert_eq!(session.project_id, project_id);
        assert!(session.active_run_id.is_none());
        let project_messages = workspace
            .messages
            .iter()
            .filter(|message| message.project_id == project_id)
            .collect::<Vec<_>>();
        assert_eq!(project_messages.len(), 3);
        assert!(
            project_messages
                .iter()
                .any(|message| message.text.as_deref() == Some(input))
        );
        assert!(
            project_messages
                .iter()
                .any(|message| message.role == "assistant"
                    && message.id == session.current_message_id)
        );
    }
}
