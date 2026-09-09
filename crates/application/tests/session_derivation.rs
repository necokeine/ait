//! Atomic Session derivation admission regressions.
#![allow(clippy::pedantic)]

mod support;

use std::sync::Arc;

use ait_application::LocalControlService;
use ait_contracts::{Command, CommandResult, ProjectView, RunView};
use ait_domain::{DomainError, ErrorCode};
use ait_ports::{WorkspaceAgent, WorkspaceAgentInvocation, WorkspaceAgentResponse};
use ait_storage_sqlite::SqliteControlStore;
use async_trait::async_trait;
use tempfile::TempDir;
use tokio::{sync::Semaphore, time::Duration};

use support::workspace;

struct ImmediateAgent;

#[async_trait]
impl WorkspaceAgent for ImmediateAgent {
    async fn invoke(
        &self,
        _request: WorkspaceAgentInvocation,
    ) -> Result<WorkspaceAgentResponse, DomainError> {
        Ok(WorkspaceAgentResponse {
            assistant_text: "fixture response".into(),
            commit_id: None,
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
impl WorkspaceAgent for BlockingAgent {
    async fn invoke(
        &self,
        _request: WorkspaceAgentInvocation,
    ) -> Result<WorkspaceAgentResponse, DomainError> {
        self.started.add_permits(1);
        self.release.acquire().await.unwrap().forget();
        Ok(WorkspaceAgentResponse {
            assistant_text: "fixture response".into(),
            commit_id: None,
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
    async fn new(agent: Arc<dyn WorkspaceAgent>) -> Self {
        let temporary = TempDir::new().unwrap();
        let workdir = temporary.path().join("project");
        std::fs::create_dir(&workdir).unwrap();
        let service = Arc::new(LocalControlService::with_workspace_agent(
            Arc::new(SqliteControlStore::in_memory().unwrap()),
            agent,
        ));
        let CommandResult::Project(project) = execute(
            &service,
            Command::RegisterProject {
                id: "project".into(),
                name: "Project".into(),
                workdir: workdir.display().to_string(),
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
        execute(
            &service,
            Command::CreateSession {
                id: "current".into(),
                project_id: project.id.clone(),
                agent_id: "agent".into(),
                at_message_id: None,
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
            agent_id: "agent".into(),
            at_message_id: self.project.root_message_id.clone(),
            text: text.into(),
        }
    }
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
        Some(fixture.project.root_message_id.as_str())
    );
}

#[tokio::test]
async fn busy_source_forks_instead_of_returning_session_busy() {
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
    assert!(response.ok, "{:?}", response.error);
    let CommandResult::Run(run) = response.result.unwrap() else {
        panic!("expected Run")
    };
    assert_eq!(run.session_id.as_deref(), Some("fork"));

    agent.started.acquire().await.unwrap().forget();
    agent.release.add_permits(1);
    wait_for_terminal(&fixture.service, &run.id).await;
    let state = workspace(&fixture.service).await;
    let user = state
        .messages
        .iter()
        .find(|message| message.text.as_deref() == Some("fork while busy"))
        .unwrap();
    assert_eq!(
        user.parent_message_id.as_deref(),
        Some(fixture.project.root_message_id.as_str())
    );
}

#[tokio::test]
async fn advanced_head_forks_from_original_source_with_correct_parent() {
    let fixture = Fixture::new(Arc::new(ImmediateAgent)).await;
    execute_run(
        &fixture.service,
        Command::SendMessage {
            session_id: "current".into(),
            text: "advance the head".into(),
        },
    )
    .await;

    let run = execute_run(&fixture.service, fixture.derive("branch from original")).await;
    assert_eq!(run.session_id.as_deref(), Some("fork"));
    let state = workspace(&fixture.service).await;
    let user = state
        .messages
        .iter()
        .find(|message| message.text.as_deref() == Some("branch from original"))
        .unwrap();
    assert_eq!(
        user.parent_message_id.as_deref(),
        Some(fixture.project.root_message_id.as_str())
    );
    let fork = state
        .sessions
        .iter()
        .find(|session| session.id == "fork")
        .unwrap();
    assert_eq!(
        state
            .messages
            .iter()
            .find(|message| message.id == fork.current_message_id)
            .unwrap()
            .parent_message_id
            .as_deref(),
        Some(user.id.as_str())
    );
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

async fn wait_for_terminal(service: &LocalControlService, run_id: &str) {
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let response = service
                .execute(Command::GetRun {
                    run_id: run_id.into(),
                })
                .await;
            let CommandResult::Run(run) = response.result.unwrap() else {
                panic!("expected Run")
            };
            if matches!(run.status.as_str(), "completed" | "failed" | "cancelled") {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("Run must finish");
}
