//! End-to-end control-plane acceptance coverage.
#![allow(clippy::pedantic)]

mod support;

use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use ait_application::LocalControlService;
use ait_contracts::{Command, CommandResult, default_settings};
use ait_domain::{DomainError, ErrorCode};
use ait_ports::{
    GeneratedSessionTitle, SessionTitleGenerator, SessionTitleRequest, WorkspaceAgent,
    WorkspaceAgentInvocation, WorkspaceAgentResponse, WorkspaceOperation, WorkspaceOutputItem,
};
use ait_storage_sqlite::SqliteControlStore;
use async_trait::async_trait;
use tempfile::TempDir;

use support::workspace;

async fn run(service: &LocalControlService, command: Command) -> CommandResult {
    let response = service.execute(command).await;
    assert!(response.ok, "{:?}", response.error);
    response.result.unwrap()
}

#[derive(Debug)]
struct FixtureCodex;

#[async_trait]
impl WorkspaceAgent for FixtureCodex {
    async fn invoke(
        &self,
        request: WorkspaceAgentInvocation,
    ) -> Result<WorkspaceAgentResponse, DomainError> {
        let assistant_text = format!("Completed: {}", request.commit_subject);
        Ok(WorkspaceAgentResponse {
            assistant_text: assistant_text.clone(),
            commit_id: None,
            operations: Vec::new(),
            output_items: vec![WorkspaceOutputItem::Message {
                id: format!("message-{}", request.request_id),
                phase: Some("final_answer".into()),
                text: assistant_text,
            }],
        })
    }
}

fn fixture_service(store: Arc<dyn ait_ports::ControlStore>) -> LocalControlService {
    LocalControlService::with_workspace_agent(store, Arc::new(FixtureCodex))
}

#[tokio::test]
async fn project_list_operations_never_return_another_projects_runtime_records() {
    let temporary = TempDir::new().unwrap();
    let service = fixture_service(Arc::new(SqliteControlStore::in_memory().unwrap()));
    run(
        &service,
        Command::RegisterAgent {
            id: "scoped-agent".into(),
            name: "Scoped agent".into(),
            config: config(),
        },
    )
    .await;
    for project_id in ["project-a", "project-b"] {
        let workdir = temporary.path().join(project_id);
        std::fs::create_dir(&workdir).unwrap();
        run(
            &service,
            Command::RegisterProject {
                id: project_id.into(),
                name: project_id.into(),
                workdir: Some(workdir.display().to_string()),
                repo_url: None,
            },
        )
        .await;
        run(
            &service,
            Command::CreateSession {
                id: format!("session-{project_id}"),
                project_id: project_id.into(),
                agent_id: "scoped-agent".into(),
                at_message_id: None,
            },
        )
        .await;
        run(
            &service,
            Command::SendMessage {
                session_id: format!("session-{project_id}"),
                text: format!("message for {project_id}"),
            },
        )
        .await;
    }

    for project_id in ["project-a", "project-b"] {
        let CommandResult::Sessions(sessions) = run(
            &service,
            Command::ListSessions {
                project_id: project_id.into(),
            },
        )
        .await
        else {
            panic!("expected Sessions")
        };
        let CommandResult::Messages(messages) = run(
            &service,
            Command::ListMessages {
                project_id: project_id.into(),
            },
        )
        .await
        else {
            panic!("expected Messages")
        };
        let CommandResult::Runs(runs) = run(
            &service,
            Command::ListRuns {
                project_id: project_id.into(),
            },
        )
        .await
        else {
            panic!("expected Runs")
        };
        assert_eq!(sessions.len(), 1);
        assert!(
            sessions
                .iter()
                .all(|session| session.project_id == project_id)
        );
        assert!(
            messages
                .iter()
                .all(|message| message.project_id == project_id)
        );
        assert_eq!(runs.len(), 1);
        assert!(runs.iter().all(|run| run.project_id == project_id));
    }
}

#[tokio::test]
async fn user_message_requires_clean_git_and_records_head_commit() {
    let temporary = TempDir::new().unwrap();
    let project_dir = temporary.path().join("project");
    std::fs::create_dir(&project_dir).unwrap();
    let service = fixture_service(Arc::new(SqliteControlStore::in_memory().unwrap()));
    let project = match run(
        &service,
        Command::RegisterProject {
            id: "git-project".into(),
            name: "Git Project".into(),
            workdir: Some(project_dir.display().to_string()),
            repo_url: Some("git@github.com:member/fork.git".into()),
        },
    )
    .await
    {
        CommandResult::Project(project) => project,
        _ => panic!(),
    };
    assert_eq!(project.base_commit.len(), 40);
    assert_eq!(
        project.repo_url.as_deref(),
        Some("git@github.com:member/fork.git")
    );
    run(
        &service,
        Command::RegisterAgent {
            id: "codex".into(),
            name: "Codex".into(),
            config: config(),
        },
    )
    .await;
    let session = run(
        &service,
        Command::CreateSession {
            id: "git-session".into(),
            project_id: project.id.clone(),
            agent_id: "codex".into(),
            at_message_id: None,
        },
    )
    .await;
    let CommandResult::Session(session) = session else {
        panic!("expected Session")
    };
    assert_eq!(
        std::path::Path::new(&session.workdir),
        project_dir
            .canonicalize()
            .unwrap()
            .join(".ait")
            .join("git-session")
    );
    assert!(
        std::path::Path::new(&session.workdir)
            .join(".git")
            .is_file()
    );
    let primary_status = std::process::Command::new("git")
        .arg("-C")
        .arg(&project_dir)
        .args(["status", "--porcelain=v1"])
        .output()
        .unwrap();
    assert!(primary_status.status.success());
    assert!(primary_status.stdout.is_empty());

    let dirty_path = std::path::Path::new(&session.workdir).join("dirty.txt");
    std::fs::write(&dirty_path, "dirty").unwrap();
    let rejected = service
        .execute(Command::SendMessage {
            session_id: "git-session".into(),
            text: "must not append".into(),
        })
        .await;
    assert_eq!(rejected.error.unwrap().code, ErrorCode::ProjectGitDirty);
    std::fs::remove_file(dirty_path).unwrap();

    run(
        &service,
        Command::SendMessage {
            session_id: "git-session".into(),
            text: "append clean input".into(),
        },
    )
    .await;
    let workspace = workspace(&service).await;
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
            operations: vec![WorkspaceOperation {
                id: "operation-1".into(),
                kind: "read".into(),
                status: "completed".into(),
                title: "Read file".into(),
                summary: None,
                detail: None,
                paths: vec!["src/main.rs".into()],
            }],
            output_items: vec![
                WorkspaceOutputItem::Message {
                    id: "commentary-1".into(),
                    phase: Some("commentary".into()),
                    text: "Inspecting the repository.".into(),
                },
                WorkspaceOutputItem::Operation {
                    id: "operation-1".into(),
                },
                WorkspaceOutputItem::Message {
                    id: "final-1".into(),
                    phase: Some("final_answer".into()),
                    text: "Implemented and verified the feature.".into(),
                },
            ],
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

#[derive(Debug)]
struct FailingTitleGenerator;

#[async_trait]
impl SessionTitleGenerator for FailingTitleGenerator {
    async fn generate(&self, _: SessionTitleRequest) -> Result<GeneratedSessionTitle, DomainError> {
        Err(DomainError::invariant(
            ErrorCode::ProviderFailed,
            "invalid Session metadata",
        ))
    }
}

#[tokio::test]
async fn first_interaction_generates_session_metadata_once_and_preserves_manual_name() {
    let temporary = TempDir::new().unwrap();
    let project_dir = temporary.path().join("project");
    std::fs::create_dir(&project_dir).unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let service = fixture_service(Arc::new(SqliteControlStore::in_memory().unwrap()))
        .with_session_title_generator(Arc::new(SuccessfulTitleGenerator {
            calls: calls.clone(),
        }));
    let project = match run(
        &service,
        Command::RegisterProject {
            id: "named-project".into(),
            name: "Named Project".into(),
            workdir: Some(project_dir.display().to_string()),
            repo_url: None,
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
            config: config(),
        },
    )
    .await;
    run(
        &service,
        Command::CreateSession {
            id: "named-session".into(),
            project_id: project.id,
            agent_id: "codex-agent".into(),
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
        },
    )
    .await;
    let pointer_version = workspace(&service).await.sessions[0].version;

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
async fn failed_title_generation_keeps_temporary_title_without_conversation_or_git_changes() {
    let temporary = TempDir::new().unwrap();
    let project_dir = temporary.path().join("project");
    std::fs::create_dir(&project_dir).unwrap();
    let service = fixture_service(Arc::new(SqliteControlStore::in_memory().unwrap()))
        .with_session_title_generator(Arc::new(FailingTitleGenerator));
    let project = match run(
        &service,
        Command::RegisterProject {
            id: "failed-title-project".into(),
            name: "Failed Title Project".into(),
            workdir: Some(project_dir.display().to_string()),
            repo_url: None,
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
            id: "failed-title-agent".into(),
            name: "Codex".into(),
            config: config(),
        },
    )
    .await;
    run(
        &service,
        Command::CreateSession {
            id: "failed-title-session".into(),
            project_id: project.id,
            agent_id: "failed-title-agent".into(),
            at_message_id: None,
        },
    )
    .await;
    run(
        &service,
        Command::SetSessionTitle {
            session_id: "failed-title-session".into(),
            title: "Temporary prompt title".into(),
        },
    )
    .await;
    run(
        &service,
        Command::SendMessage {
            session_id: "failed-title-session".into(),
            text: "first interaction".into(),
        },
    )
    .await;

    let before = workspace(&service).await;
    let head_before = std::process::Command::new("git")
        .arg("-C")
        .arg(&project_dir)
        .args(["rev-parse", "HEAD"])
        .output()
        .unwrap()
        .stdout;
    let response = service
        .generate_session_title("failed-title-session".into(), "first interaction".into())
        .await;

    assert!(!response.ok);
    assert_eq!(response.error.unwrap().code, ErrorCode::ProviderFailed);
    let after = workspace(&service).await;
    let session = after
        .sessions
        .iter()
        .find(|session| session.id == "failed-title-session")
        .unwrap();
    assert_eq!(session.title.as_deref(), Some("Temporary prompt title"));
    assert!(session.description.is_empty());
    assert!(session.title_generation_started);
    assert_eq!(after.messages, before.messages);
    assert_eq!(after.runs, before.runs);
    let head_after = std::process::Command::new("git")
        .arg("-C")
        .arg(&project_dir)
        .args(["rev-parse", "HEAD"])
        .output()
        .unwrap()
        .stdout;
    assert_eq!(head_after, head_before);
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
            workdir: Some(project_dir.display().to_string()),
            repo_url: None,
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
            config: config(),
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
        },
    )
    .await
    {
        CommandResult::Run(value) => value,
        _ => panic!(),
    };
    assert_eq!(completed.status, "completed");
    assert_eq!(completed.config.reasoning_effort.as_deref(), Some("high"));

    let workspace = workspace(&service).await;
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
    assert_eq!(
        assistant.data.as_ref().unwrap()["codex"]["operations"][0]["paths"][0],
        serde_json::json!("src/main.rs")
    );
    assert_eq!(
        assistant.data.as_ref().unwrap()["codex"]["output_items"],
        serde_json::json!([
            {
                "type": "message",
                "id": "commentary-1",
                "phase": "commentary",
                "text": "Inspecting the repository."
            },
            { "type": "operation", "id": "operation-1" },
            {
                "type": "message",
                "id": "final-1",
                "phase": "final_answer",
                "text": "Implemented and verified the feature."
            }
        ])
    );
}

#[tokio::test]
async fn idle_session_can_rebind_between_named_agents() {
    let temporary = TempDir::new().unwrap();
    let project_dir = temporary.path().join("project");
    std::fs::create_dir(&project_dir).unwrap();
    let service = fixture_service(Arc::new(SqliteControlStore::in_memory().unwrap()));
    let project = match run(
        &service,
        Command::RegisterProject {
            id: "rebind-project".into(),
            name: "Rebind Project".into(),
            workdir: Some(project_dir.display().to_string()),
            repo_url: None,
        },
    )
    .await
    {
        CommandResult::Project(value) => value,
        _ => panic!(),
    };
    for id in ["primary-agent", "alternate-agent"] {
        run(
            &service,
            Command::RegisterAgent {
                id: id.into(),
                name: id.into(),
                config: config(),
            },
        )
        .await;
    }
    run(
        &service,
        Command::CreateSession {
            id: "rebind-session".into(),
            project_id: project.id,
            agent_id: "primary-agent".into(),
            at_message_id: None,
        },
    )
    .await;
    let rebound = match run(
        &service,
        Command::SetSessionAgent {
            session_id: "rebind-session".into(),
            agent_id: "alternate-agent".into(),
        },
    )
    .await
    {
        CommandResult::Session(value) => value,
        _ => panic!(),
    };
    assert_eq!(rebound.agent_id, "alternate-agent");
    assert_eq!(rebound.version, 2);

    let completed = match run(
        &service,
        Command::SendMessage {
            session_id: "rebind-session".into(),
            text: "run with the alternate agent".into(),
        },
    )
    .await
    {
        CommandResult::Run(value) => value,
        _ => panic!(),
    };
    assert_eq!(completed.agent_id, "alternate-agent");
    assert_eq!(completed.status, "completed");
}

#[tokio::test]
async fn codex_session_branch_cron_events_and_restart_form_one_vertical_slice() {
    let started_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    let temporary = TempDir::new().unwrap();
    let database = temporary.path().join("ait.sqlite3");
    let project_dir = temporary.path().join("project");
    std::fs::create_dir(&project_dir).unwrap();
    let service = fixture_service(Arc::new(
        ait_storage_sqlite::SplitSqliteControlStore::open(&database).unwrap(),
    ));

    let project = match run(
        &service,
        Command::RegisterProject {
            id: "project-1".into(),
            name: "Demo".into(),
            workdir: Some(project_dir.display().to_string()),
            repo_url: None,
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
            id: "agent-codex".into(),
            name: "Codex agent".into(),
            config: config(),
        },
    )
    .await;
    run(
        &service,
        Command::CreateSession {
            id: "session-main".into(),
            project_id: project.id.clone(),
            agent_id: "agent-codex".into(),
            at_message_id: None,
        },
    )
    .await;
    let interactive = match run(
        &service,
        Command::SendMessage {
            session_id: "session-main".into(),
            text: "use the echo tool".into(),
        },
    )
    .await
    {
        CommandResult::Run(value) => value,
        _ => panic!(),
    };
    assert_eq!(interactive.status, "completed");
    assert_eq!(
        interactive.workspace_base_commit.as_deref(),
        Some(project.base_commit.as_str())
    );

    run(
        &service,
        Command::CreateSession {
            id: "session-branch".into(),
            project_id: project.id.clone(),
            agent_id: "agent-codex".into(),
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
            agent_id: "agent-codex".into(),
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
    assert_eq!(
        scheduled.workspace_base_commit.as_deref(),
        Some(project.base_commit.as_str())
    );

    let first_page = service.replay_events(0, 3).await.unwrap();
    assert_eq!(first_page.len(), 3);
    let remainder = service
        .replay_events(first_page.last().unwrap().cursor, 100)
        .await
        .unwrap();
    assert!(!remainder.is_empty());
    assert!(remainder[0].cursor > first_page.last().unwrap().cursor);

    let before_restart = workspace(&service).await;
    let finished_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    assert!(
        before_restart
            .messages
            .iter()
            .all(|message| { (started_at..=finished_at).contains(&message.created_at) })
    );
    assert!(before_restart.messages.iter().any(|message| {
        message
            .data
            .as_ref()
            .is_some_and(|data| data["codex"]["output_items"][0]["phase"] == "final_answer")
    }));

    drop(service);
    let recovered = LocalControlService::new(Arc::new(
        ait_storage_sqlite::SplitSqliteControlStore::open(&database).unwrap(),
    ));
    let workspace = workspace(&recovered).await;
    assert_eq!(workspace.runs.len(), 2);
    assert_eq!(workspace.messages, before_restart.messages);
    assert_eq!(workspace.sessions.len(), 2);
    assert_eq!(workspace.messages.len(), 4);
    assert!(workspace.runs.iter().all(|run| run.status == "completed"));
}

#[cfg(unix)]
#[tokio::test]
async fn session_worktree_paths_reject_traversal_and_symbolic_link_parents() {
    use std::os::unix::fs::symlink;

    let temporary = TempDir::new().unwrap();
    let project_dir = temporary.path().join("project");
    let outside = temporary.path().join("outside");
    std::fs::create_dir(&project_dir).unwrap();
    std::fs::create_dir(&outside).unwrap();
    let service = fixture_service(Arc::new(SqliteControlStore::in_memory().unwrap()));
    run(
        &service,
        Command::RegisterProject {
            id: "safe-project".into(),
            name: "Safe Project".into(),
            workdir: Some(project_dir.display().to_string()),
            repo_url: None,
        },
    )
    .await;
    run(
        &service,
        Command::RegisterAgent {
            id: "safe-agent".into(),
            name: "Safe Agent".into(),
            config: config(),
        },
    )
    .await;

    for id in [
        "../escape",
        "project.sqlite3",
        "project.sqlite3-wal",
        "project.sqlite3-shm",
        "project.sqlite3-journal",
        "PROJECT.SQLITE3-WAL",
        "project.sqlite3-shm. ",
    ] {
        let rejected = service
            .execute(Command::CreateSession {
                id: id.into(),
                project_id: "safe-project".into(),
                agent_id: "safe-agent".into(),
                at_message_id: None,
            })
            .await;
        assert_eq!(
            rejected.error.unwrap().code,
            ErrorCode::InvalidSession,
            "{id}"
        );
    }
    assert!(!temporary.path().join("escape").exists());
    assert!(!project_dir.join(".ait").exists());

    symlink(&outside, project_dir.join(".ait")).unwrap();
    let linked_parent = service
        .execute(Command::CreateSession {
            id: "safe-session".into(),
            project_id: "safe-project".into(),
            agent_id: "safe-agent".into(),
            at_message_id: None,
        })
        .await;
    assert_eq!(linked_parent.error.unwrap().code, ErrorCode::InvalidSession);
    assert!(!outside.join("safe-session").exists());

    let CommandResult::Sessions(sessions) = run(
        &service,
        Command::ListSessions {
            project_id: "safe-project".into(),
        },
    )
    .await
    else {
        panic!("expected Sessions")
    };
    assert!(sessions.is_empty());
}

#[tokio::test]
async fn retired_builtin_configs_are_rejected_and_provider_failures_are_persisted() {
    let temporary = TempDir::new().unwrap();
    let project_dir = temporary.path().join("project");
    std::fs::create_dir(&project_dir).unwrap();
    #[derive(Debug)]
    struct FailingCodex;
    #[async_trait]
    impl WorkspaceAgent for FailingCodex {
        async fn invoke(
            &self,
            _: WorkspaceAgentInvocation,
        ) -> Result<WorkspaceAgentResponse, DomainError> {
            Err(DomainError::transient(
                ErrorCode::ProviderFailed,
                "fixture provider failure",
            ))
        }
    }
    let service = LocalControlService::with_workspace_agent(
        Arc::new(SqliteControlStore::in_memory().unwrap()),
        Arc::new(FailingCodex),
    );
    let invalid = service
        .execute(Command::RegisterAgent {
            id: "bad".into(),
            name: "".into(),
            config: config(),
        })
        .await;
    assert_eq!(
        invalid.error.unwrap().code,
        ErrorCode::InvalidAgentConfiguration
    );
    for provider_id in [
        "builtin-tool",
        "builtin-manual",
        "builtin-provider_failure",
        "builtin-approval_required",
    ] {
        let retired = service
            .execute(Command::RegisterAgent {
                id: format!("retired-{provider_id}"),
                name: "Retired".into(),
                config: ait_contracts::AgentConfiguration {
                    provider_id: provider_id.into(),
                    model: "default".into(),
                    reasoning_effort: None,
                },
            })
            .await;
        assert_eq!(
            retired.error.unwrap().code,
            ErrorCode::InvalidAgentConfiguration
        );
    }
    let project = match run(
        &service,
        Command::RegisterProject {
            id: "p".into(),
            name: "P".into(),
            workdir: Some(project_dir.display().to_string()),
            repo_url: None,
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
            id: "codex".into(),
            name: "Codex".into(),
            config: config(),
        },
    )
    .await;
    run(
        &service,
        Command::CreateSession {
            id: "session".into(),
            project_id: project.id,
            agent_id: "codex".into(),
            at_message_id: None,
        },
    )
    .await;
    let provider = match run(
        &service,
        Command::SendMessage {
            session_id: "session".into(),
            text: "go".into(),
        },
    )
    .await
    {
        CommandResult::Run(value) => value,
        _ => panic!(),
    };
    assert_eq!(provider.status, "failed");
    assert_eq!(
        provider.error.as_ref().unwrap().code,
        ErrorCode::ProviderFailed
    );
    assert!(provider.error.unwrap().retryable);
}

#[tokio::test]
async fn project_export_import_preserves_tree_and_revisions_without_runtime_or_credentials() {
    let temporary = TempDir::new().unwrap();
    let source_dir = temporary.path().join("source");
    let imported_dir = temporary.path().join("imported");
    std::fs::create_dir(&source_dir).unwrap();
    std::fs::create_dir(&imported_dir).unwrap();
    let source = fixture_service(Arc::new(SqliteControlStore::in_memory().unwrap()));
    let project = match run(
        &source,
        Command::RegisterProject {
            id: "portable-project".into(),
            name: "Portable".into(),
            workdir: Some(source_dir.display().to_string()),
            repo_url: None,
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
            config: config(),
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
    assert_eq!(archive.sessions[0].version, 4);
    assert!(archive.sessions[0].active_run_id.is_none());
    assert!(archive.sessions[0].workdir.is_empty());
    assert_eq!(archive.messages.len(), 3);
    assert!(
        archive
            .messages
            .iter()
            .all(|message| message.created_at > 0)
    );

    let target = LocalControlService::new(Arc::new(SqliteControlStore::in_memory().unwrap()));
    run(
        &target,
        Command::ImportProject {
            archive: archive.clone(),
            workdir: imported_dir.display().to_string(),
        },
    )
    .await;
    let workspace = workspace(&target).await;
    assert_eq!(workspace.projects[0].revision, archive.project.revision);
    assert_eq!(
        workspace.projects[0].default_agent_id,
        archive.project.default_agent_id
    );
    assert_eq!(workspace.agents[0].revision, archive.agents[0].revision);
    assert_eq!(workspace.sessions[0].version, archive.sessions[0].version);
    assert_eq!(
        std::path::Path::new(&workspace.sessions[0].workdir),
        imported_dir
            .canonicalize()
            .unwrap()
            .join(".ait")
            .join("portable-session")
    );
    assert!(
        std::path::Path::new(&workspace.sessions[0].workdir)
            .join(".git")
            .is_file()
    );
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
    let service = fixture_service(Arc::new(
        ait_storage_sqlite::SplitSqliteControlStore::open(&database).unwrap(),
    ));
    let project = match run(
        &service,
        Command::RegisterProject {
            id: "desktop-project".into(),
            name: "Desktop".into(),
            workdir: Some(project_dir.display().to_string()),
            repo_url: None,
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
            config: config(),
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

    let recovered = LocalControlService::new(Arc::new(
        ait_storage_sqlite::SplitSqliteControlStore::open(&database).unwrap(),
    ));
    let workspace = workspace(&recovered).await;
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
    let service = fixture_service(Arc::new(SqliteControlStore::in_memory().unwrap()));

    run(
        &service,
        Command::RegisterAgent {
            id: "tool-local".into(),
            name: "Tool".into(),
            config: config(),
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
                workdir: Some(directory.display().to_string()),
                repo_url: None,
            },
        )
        .await;
        run(
            &service,
            Command::SetProjectDefaultAgent {
                project_id: id.into(),
                agent_id: "tool-local".into(),
            },
        )
        .await;
        run(
            &service,
            Command::CreateSession {
                id: session_id.into(),
                project_id: id.into(),
                agent_id: "tool-local".into(),
                at_message_id: None,
            },
        )
        .await;
        let completed = match run(
            &service,
            Command::SendMessage {
                session_id: session_id.into(),
                text: input.into(),
            },
        )
        .await
        {
            CommandResult::Run(value) => value,
            _ => panic!(),
        };
        assert_eq!(completed.status, "completed");
    }

    let workspace = workspace(&service).await;
    assert_eq!(workspace.projects.len(), 2);
    assert!(workspace.projects.iter().all(|project| {
        project.default_agent_id.as_deref() == Some("tool-local") && project.revision == 2
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

fn config() -> ait_contracts::AgentConfiguration {
    ait_contracts::AgentConfiguration {
        provider_id: "builtin-codex".into(),
        model: "gpt-5.6-sol".into(),
        reasoning_effort: Some("high".into()),
    }
}
