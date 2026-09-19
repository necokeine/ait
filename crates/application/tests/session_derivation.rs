//! Atomic Session derivation admission regressions.
#![allow(clippy::pedantic)]

use crate::support::native::{NativeHandler, NativeReply};
mod support;

use std::sync::Arc;

use ait_application::LocalControlService;
use ait_contracts::{Command, CommandResult, ProjectView, RunView, default_settings};
use ait_domain::{DomainError, ErrorCode, SessionStatus};
use ait_ports::CodexThreadInvocation;
use ait_storage_sqlite::SqliteControlStore;
use async_trait::async_trait;
use tempfile::TempDir;
use tokio::{sync::Semaphore, time::Duration};

use support::workspace;

struct ImmediateAgent;

#[async_trait]
impl NativeHandler for ImmediateAgent {
    async fn invoke(&self, _request: CodexThreadInvocation) -> Result<NativeReply, DomainError> {
        Ok(NativeReply {
            assistant_text: "fixture response".into(),

            operations: Vec::new(),
            output_items: Vec::new(),
        })
    }
}

struct BlockingAgent {
    started: Semaphore,
    release: Semaphore,
}

#[async_trait]
impl NativeHandler for BlockingAgent {
    async fn invoke(&self, _request: CodexThreadInvocation) -> Result<NativeReply, DomainError> {
        self.started.add_permits(1);
        self.release.acquire().await.unwrap().forget();
        Ok(NativeReply {
            assistant_text: "fixture response".into(),

            operations: Vec::new(),
            output_items: Vec::new(),
        })
    }
}

struct Fixture {
    _temporary: TempDir,
    service: Arc<LocalControlService>,
    project: ProjectView,
}

impl Fixture {
    async fn new(agent: Arc<dyn NativeHandler>) -> Self {
        let fixture = Self::empty(agent).await;
        execute(
            &fixture.service,
            Command::CreateSession {
                id: "current".into(),
                project_id: fixture.project.id.clone(),
                agent_id: "agent".into(),
                at_message_id: None,
            },
        )
        .await;
        fixture
    }

    async fn empty(agent: Arc<dyn NativeHandler>) -> Self {
        let temporary = TempDir::new().unwrap();
        let workdir = temporary.path().join("project");
        std::fs::create_dir(&workdir).unwrap();
        let service = Arc::new(crate::support::native::native_service(
            std::sync::Arc::new(ait_workspace_local::LocalProjectWorkspace::default()),
            Arc::new(SqliteControlStore::in_memory().unwrap()),
            agent,
        ));
        let CommandResult::Project(project) = execute(
            &service,
            Command::RegisterProject {
                id: "project".into(),
                name: "Project".into(),
                workdir: Some(workdir.display().to_string()),
                repo_url: None,
            },
        )
        .await
        else {
            panic!("expected Project")
        };
        execute(
            &service,
            Command::RegisterAgent {
                id: "agent".into(),
                name: "Agent".into(),
                config: config(),
            },
        )
        .await;
        let mut settings = default_settings();
        settings
            .0
            .insert("agents.default_agent".into(), serde_json::json!("agent"));
        execute(
            &service,
            Command::SaveSettings {
                expected_revision: 1,
                values: settings,
            },
        )
        .await;
        Self {
            _temporary: temporary,
            service,
            project,
        }
    }

    fn derive(&self, text: &str) -> Command {
        Command::DeriveSession {
            id: "fork".into(),
            project_id: self.project.id.clone(),
            source_session_id: "current".into(),
            agent_id: String::new(),
            at_message_id: self.project.root_message_id.clone(),
            text: text.into(),
        }
    }
}

#[tokio::test]
async fn first_input_atomically_forks_from_the_initial_system_message_without_a_source_session() {
    let fixture = Fixture::empty(Arc::new(ImmediateAgent)).await;
    let initial = workspace(&fixture.service).await;
    assert!(initial.sessions.is_empty());
    assert!(initial.runs.is_empty());
    assert_eq!(initial.messages.len(), 1);

    let fork = |text: &str| Command::ForkSession {
        id: "first-session".into(),
        project_id: fixture.project.id.clone(),
        agent_id: "agent".into(),
        at_message_id: fixture.project.root_message_id.clone(),
        text: text.into(),
    };
    let rejected = fixture.service.execute(fork("   ")).await;
    assert!(!rejected.ok);
    let after_rejection = workspace(&fixture.service).await;
    assert!(after_rejection.sessions.is_empty());
    assert!(after_rejection.runs.is_empty());
    assert_eq!(after_rejection.messages.len(), 1);

    let run = execute_run(&fixture.service, fork("First input")).await;
    assert_eq!(run.session_id.as_deref(), Some("first-session"));
    let accepted = workspace(&fixture.service).await;
    assert_eq!(accepted.sessions.len(), 1);
    assert_eq!(accepted.runs.len(), 1);
    let user = accepted
        .messages
        .iter()
        .find(|message| message.text.as_deref() == Some("First input"))
        .unwrap();
    assert_eq!(user.text.as_deref(), Some("First input"));
    assert_eq!(
        user.parent_message_id.as_deref(),
        Some(run.base_message_id.as_str())
    );
    let root = accepted
        .messages
        .iter()
        .find(|message| message.id == fixture.project.root_message_id)
        .unwrap();
    assert_eq!(root, &initial.messages[0]);

    // A transport retry keeps the candidate Session ID. Even after the first
    // Run has finished, replay cannot create another Session, input, or Run.
    let replay = fixture.service.execute(fork("First input")).await;
    assert!(!replay.ok);
    let after_replay = workspace(&fixture.service).await;
    assert_eq!(after_replay.sessions, accepted.sessions);
    assert_eq!(after_replay.runs, accepted.runs);
    assert_eq!(after_replay.messages, accepted.messages);
}

async fn execute(service: &LocalControlService, command: Command) -> CommandResult {
    let response = service.execute(command).await;
    assert!(response.ok, "{:?}", response.error);
    response.result.unwrap()
}

async fn execute_run(service: &LocalControlService, command: Command) -> RunView {
    let CommandResult::Run(run) = execute(service, command).await else {
        panic!("expected Run")
    };
    run
}

fn config() -> ait_contracts::AgentConfiguration {
    ait_contracts::AgentConfiguration {
        provider_id: "builtin-codex".into(),
        model: "gpt-5.6-sol".into(),
        reasoning_effort: Some("high".into()),
        system_prompt: None,
    }
}

#[tokio::test]
async fn idle_unchanged_source_reuses_current_session() {
    let fixture = Fixture::new(Arc::new(ImmediateAgent)).await;
    let run = execute_run(&fixture.service, fixture.derive("continue here")).await;
    assert_eq!(run.session_id.as_deref(), Some("current"));

    let state = workspace(&fixture.service).await;
    assert_eq!(state.sessions.len(), 1);
    let user = state
        .messages
        .iter()
        .find(|message| message.text.as_deref() == Some("continue here"))
        .unwrap();
    assert_eq!(
        user.parent_message_id.as_deref(),
        Some(run.base_message_id.as_str())
    );
}

#[tokio::test]
async fn archived_native_source_derives_a_new_active_session_without_reuse() {
    let fixture = Fixture::new(Arc::new(ImmediateAgent)).await;
    execute(
        &fixture.service,
        Command::SetSessionArchived {
            session_id: "current".into(),
            archived: true,
        },
    )
    .await;

    let run = execute_run(&fixture.service, fixture.derive("branch from archive")).await;
    assert_eq!(run.session_id.as_deref(), Some("fork"));

    let state = workspace(&fixture.service).await;
    assert_eq!(state.sessions.len(), 2);
    let archived = state
        .sessions
        .iter()
        .find(|session| session.id == "current")
        .unwrap();
    assert_eq!(archived.status, SessionStatus::Archived);
    assert_eq!(archived.current_message_id, fixture.project.root_message_id);
    let derived = state
        .sessions
        .iter()
        .find(|session| session.id == "fork")
        .unwrap();
    assert_eq!(derived.status, SessionStatus::Active);
    assert_eq!(
        derived.current_message_id,
        run.last_message_id.clone().unwrap()
    );
}

#[tokio::test]
async fn busy_native_source_requires_explicit_native_fork() {
    let agent = Arc::new(BlockingAgent {
        started: Semaphore::new(0),
        release: Semaphore::new(0),
    });
    let fixture = Fixture::new(agent.clone()).await;
    let accepted = fixture
        .service
        .submit(Command::SendMessage {
            session_id: "current".into(),
            text: "occupy source".into(),
        })
        .await;
    assert!(accepted.ok, "{:?}", accepted.error);
    agent.started.acquire().await.unwrap().forget();

    let service = fixture.service.clone();
    let derivation = fixture.derive("fork while busy");
    let pending = tokio::spawn(async move { service.submit(derivation).await });
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(!pending.is_finished());
    agent.release.add_permits(1);

    let response = tokio::time::timeout(Duration::from_secs(2), pending)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        response.error.unwrap().code,
        ait_domain::ErrorCode::CodexForkBoundaryUnsupported
    );
    let state = workspace(&fixture.service).await;
    assert_eq!(state.sessions.len(), 1);
    assert!(
        state
            .messages
            .iter()
            .all(|message| message.text.as_deref() != Some("fork while busy"))
    );
}

#[tokio::test]
async fn advanced_native_head_rejects_copied_history_fallback() {
    let fixture = Fixture::new(Arc::new(ImmediateAgent)).await;
    execute_run(
        &fixture.service,
        Command::SendMessage {
            session_id: "current".into(),
            text: "advance the head".into(),
        },
    )
    .await;

    let before = workspace(&fixture.service).await;
    let rejected = fixture
        .service
        .execute(fixture.derive("branch from original"))
        .await;
    assert_eq!(
        rejected.error.unwrap().code,
        ait_domain::ErrorCode::CodexForkBoundaryUnsupported
    );
    assert_eq!(workspace(&fixture.service).await, before);
}

#[tokio::test]
async fn non_concurrency_errors_are_not_hidden_by_fork_fallback() {
    let fixture = Fixture::new(Arc::new(ImmediateAgent)).await;
    let Command::DeriveSession {
        id,
        project_id,
        source_session_id,
        at_message_id,
        text,
        ..
    } = fixture.derive("must fail")
    else {
        unreachable!()
    };
    let response = fixture
        .service
        .execute(Command::DeriveSession {
            id,
            project_id,
            source_session_id,
            agent_id: "missing-agent".into(),
            at_message_id,
            text,
        })
        .await;
    assert_eq!(
        response.error.as_ref().map(|error| error.code),
        Some(ErrorCode::InvalidAgentConfiguration)
    );
    assert_eq!(workspace(&fixture.service).await.sessions.len(), 1);
}
