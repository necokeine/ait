//! Session exclusion, configuration ownership and provider credential boundaries.
#![allow(clippy::pedantic)]

mod support;

use ait_agent_adapters::{
    AdapterError, AgentAdapter, AgentCapabilities, AgentEvent, AgentRunRequest, AgentRunStatus,
    AgentStream, codex::CodexWorkspaceAgent,
};
use ait_application::{LocalControlService, PermissionPolicyLimits};
use ait_contracts::{
    AgentConfiguration, AgentMode, AgentProvider, Command, CommandResult, NativeApprovalAction,
    ProviderModel, ProviderSecret, RunView, default_settings,
};
use ait_domain::{
    ApprovalGrantScope, ApprovalMode, DomainError, ErrorCode, NativeApprovalKind,
    NativeApprovalTarget, RunPermissionProfile, SandboxAccess,
};
use ait_ports::{
    AgentProviderGateway, ControlChange, ControlFilter, ControlRead, ControlRecord,
    ControlRecordKind, ControlStore, ControlStoreError, DurableEvent, HostProviderModelCatalog,
    PendingEvent, ProviderMessage, WorkspaceAgent, WorkspaceAgentInvocation,
    WorkspaceAgentResponse, WorkspaceApprovalDecision, WorkspaceApprovalRequest,
    WorkspaceProgressReporter, WorkspaceResultSink,
};
use ait_storage_sqlite::SqliteControlStore;
use async_trait::async_trait;
use std::{
    collections::{HashMap, VecDeque},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::sync::Semaphore;

use support::{ControlStoreTestExt, WorkspaceView, terminal_run_status, workspace};

#[derive(Debug)]
struct ReadOnlyViolatingAdapter;

#[async_trait]
impl AgentAdapter for ReadOnlyViolatingAdapter {
    fn driver(&self) -> &'static str {
        "read_only_violation_test"
    }

    fn capabilities(&self) -> AgentCapabilities {
        AgentCapabilities {
            streaming: true,
            thread_resume: false,
            approvals: false,
            command_execution: true,
            file_changes: true,
            usage: false,
        }
    }

    async fn run(&self, request: AgentRunRequest) -> Result<AgentStream, AdapterError> {
        std::fs::write(request.cwd.join("unauthorized.txt"), "must not escape\n").unwrap();
        Ok(Box::pin(futures_util::stream::iter([
            Ok(AgentEvent::ItemCompleted {
                item: serde_json::json!({
                    "type": "agentMessage",
                    "id": "final",
                    "phase": "final_answer",
                    "text": "Wrote a file."
                }),
            }),
            Ok(AgentEvent::Completed {
                turn_id: "turn-read-only".into(),
                status: AgentRunStatus::Completed,
                error: None,
            }),
        ])))
    }
}

fn config(effort: &str) -> AgentConfiguration {
    AgentConfiguration {
        provider_id: "builtin-codex".into(),
        model: "gpt-5.6-sol".into(),
        reasoning_effort: Some(effort.into()),
    }
}
fn send(id: &str) -> Command {
    Command::SendMessage {
        session_id: id.into(),
        text: "hello".into(),
    }
}
fn send_text(id: &str, text: &str) -> Command {
    Command::SendMessage {
        session_id: id.into(),
        text: text.into(),
    }
}
async fn ok(service: &LocalControlService, command: Command) -> CommandResult {
    let response = service.execute(command).await;
    assert!(response.ok, "{:?}", response.error);
    response.result.unwrap()
}
async fn view(service: &LocalControlService) -> WorkspaceView {
    workspace(service).await
}

async fn submit_run(service: &Arc<LocalControlService>, command: Command) -> RunView {
    let response = service.submit(command).await;
    assert!(response.ok, "{:?}", response.error);
    let CommandResult::Run(run) = response.result.unwrap() else {
        panic!("expected Run")
    };
    run
}

async fn wait_for_signal(semaphore: &Semaphore) {
    tokio::time::timeout(Duration::from_secs(3), semaphore.acquire())
        .await
        .unwrap()
        .unwrap()
        .forget();
}
async fn setup(
    service: &LocalControlService,
    configuration: AgentConfiguration,
) -> tempfile::TempDir {
    let directory = tempfile::tempdir().unwrap();
    ok(
        service,
        Command::RegisterProject {
            id: "p".into(),
            name: "Project".into(),
            workdir: directory.path().display().to_string(),
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

struct BlockingAgent {
    entered: Semaphore,
    release: Semaphore,
    requests: Mutex<Vec<(String, Option<String>)>>,
}
impl BlockingAgent {
    fn new() -> Self {
        Self {
            entered: Semaphore::new(0),
            release: Semaphore::new(0),
            requests: Mutex::default(),
        }
    }
    async fn started(&self) {
        tokio::time::timeout(Duration::from_secs(3), self.entered.acquire())
            .await
            .unwrap()
            .unwrap()
            .forget();
    }
}
#[async_trait]
impl WorkspaceAgent for BlockingAgent {
    async fn invoke(
        &self,
        request: WorkspaceAgentInvocation,
    ) -> Result<WorkspaceAgentResponse, DomainError> {
        let cancellation = request.cancellation.clone();
        self.requests
            .lock()
            .unwrap()
            .push((request.model, request.reasoning_effort));
        self.entered.add_permits(1);
        tokio::select! {
            permit = self.release.acquire() => permit.unwrap().forget(),
            () = cancellation.cancelled() => {
                return Err(DomainError::invariant(ErrorCode::RunCancelled, "cancelled"));
            }
        }
        Ok(WorkspaceAgentResponse {
            assistant_text: "done".into(),
            commit_id: None,
            operations: Vec::new(),
            output_items: Vec::new(),
        })
    }
}

struct CommittingAgent {
    entered: Semaphore,
    release: Semaphore,
    commits: Mutex<Vec<(String, String, String)>>,
}

impl CommittingAgent {
    fn new() -> Self {
        Self {
            entered: Semaphore::new(0),
            release: Semaphore::new(0),
            commits: Mutex::default(),
        }
    }

    async fn started(&self) {
        self.entered.acquire().await.unwrap().forget();
    }
}

fn git_head(path: &std::path::Path) -> String {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(path)
        .args(["rev-parse", "HEAD"])
        .output()
        .unwrap();
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

fn git_index_tree(path: &std::path::Path) -> String {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(path)
        .arg("write-tree")
        .output()
        .unwrap();
    assert!(output.status.success());
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

fn git_commit_tree(path: &std::path::Path, commit: &str) -> String {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(path)
        .args(["rev-parse", &format!("{commit}^{{tree}}")])
        .output()
        .unwrap();
    assert!(output.status.success());
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

#[async_trait]
impl WorkspaceAgent for CommittingAgent {
    async fn invoke(
        &self,
        request: WorkspaceAgentInvocation,
    ) -> Result<WorkspaceAgentResponse, DomainError> {
        assert_eq!(git_head(&request.cwd), request.baseline_commit);
        self.entered.add_permits(1);
        self.release.acquire().await.unwrap().forget();
        let file = if request.commit_subject == "first change" {
            "first.txt"
        } else {
            "second.txt"
        };
        std::fs::write(request.cwd.join(file), format!("{file}\n")).unwrap();
        assert!(
            std::process::Command::new("git")
                .arg("-C")
                .arg(&request.cwd)
                .args(["add", "--", file])
                .status()
                .unwrap()
                .success()
        );
        assert!(
            std::process::Command::new("git")
                .arg("-C")
                .arg(&request.cwd)
                .args([
                    "-c",
                    "user.name=Test Agent",
                    "-c",
                    "user.email=test-agent@example.invalid",
                    "commit",
                    "-m",
                    &request.commit_subject,
                ])
                .status()
                .unwrap()
                .success()
        );
        let commit = git_head(&request.cwd);
        self.commits
            .lock()
            .unwrap()
            .push((request.baseline_commit, commit.clone(), file.into()));
        Ok(WorkspaceAgentResponse {
            assistant_text: format!("created {file}"),
            commit_id: Some(commit),
            operations: Vec::new(),
            output_items: Vec::new(),
        })
    }
}

struct SettlingCancellationAgent {
    calls: std::sync::atomic::AtomicUsize,
    entered: Semaphore,
    cancelling: Semaphore,
    settle_release: Semaphore,
}

impl SettlingCancellationAgent {
    fn new() -> Self {
        Self {
            calls: std::sync::atomic::AtomicUsize::new(0),
            entered: Semaphore::new(0),
            cancelling: Semaphore::new(0),
            settle_release: Semaphore::new(0),
        }
    }

    async fn started(&self) {
        self.entered.acquire().await.unwrap().forget();
    }
}

#[async_trait]
impl WorkspaceAgent for SettlingCancellationAgent {
    async fn invoke(
        &self,
        request: WorkspaceAgentInvocation,
    ) -> Result<WorkspaceAgentResponse, DomainError> {
        let call = self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.entered.add_permits(1);
        if call == 0 {
            request.cancellation.cancelled().await;
            self.cancelling.add_permits(1);
            self.settle_release.acquire().await.unwrap().forget();
            return Err(DomainError::invariant(
                ErrorCode::RunCancelled,
                "cancelled after workspace settlement",
            ));
        }
        Ok(WorkspaceAgentResponse {
            assistant_text: "second run completed".into(),
            commit_id: None,
            operations: Vec::new(),
            output_items: Vec::new(),
        })
    }
}

struct FinalizationRaceAgent {
    entered: Semaphore,
    begin_integration: Semaphore,
    integration_started: Semaphore,
    complete: Semaphore,
}

impl FinalizationRaceAgent {
    fn new() -> Self {
        Self {
            entered: Semaphore::new(0),
            begin_integration: Semaphore::new(0),
            integration_started: Semaphore::new(0),
            complete: Semaphore::new(0),
        }
    }

    async fn started(&self) {
        self.entered.acquire().await.unwrap().forget();
    }
}

#[async_trait]
impl WorkspaceAgent for FinalizationRaceAgent {
    async fn invoke(
        &self,
        _request: WorkspaceAgentInvocation,
    ) -> Result<WorkspaceAgentResponse, DomainError> {
        panic!("FinalizationRaceAgent requires the checkpointed invocation boundary")
    }

    async fn invoke_with_progress_and_checkpoint(
        &self,
        request: WorkspaceAgentInvocation,
        _progress: Arc<dyn WorkspaceProgressReporter>,
        result_sink: &dyn WorkspaceResultSink,
    ) -> Result<WorkspaceAgentResponse, DomainError> {
        self.entered.add_permits(1);
        self.begin_integration.acquire().await.unwrap().forget();
        let result = WorkspaceAgentResponse {
            assistant_text: "integrated result".into(),
            commit_id: Some("fixture-commit".into()),
            operations: Vec::new(),
            output_items: Vec::new(),
        };
        result_sink.checkpoint(result.clone()).await?;
        request
            .integration_gate
            .as_ref()
            .expect("application integration gate")
            .begin_integration()
            .await?;
        self.integration_started.add_permits(1);
        self.complete.acquire().await.unwrap().forget();
        Ok(result)
    }
}

struct TerminalFailingStore {
    inner: SqliteControlStore,
    failure_status: &'static str,
    pause_status: &'static str,
    failures: Mutex<VecDeque<ControlStoreError>>,
    pause_once: AtomicBool,
    failures_exhausted: Semaphore,
    allow_terminal_commit: Semaphore,
    terminal_committed: Semaphore,
}

#[async_trait]
impl ControlStore for TerminalFailingStore {
    async fn read(&self, filters: &[ControlFilter]) -> Result<ControlRead, ControlStoreError> {
        self.inner.read(filters).await
    }

    async fn apply(
        &self,
        revision: u64,
        changes: Vec<ControlChange>,
        events: Vec<PendingEvent>,
    ) -> Result<u64, ControlStoreError> {
        let status = terminal_run_status(&changes).map(str::to_owned);
        if status.as_deref() == Some(self.failure_status) {
            let injected = self.failures.lock().unwrap().pop_front();
            if let Some(failure) = injected {
                return Err(failure);
            }
        }
        if status.as_deref() == Some(self.pause_status)
            && self.pause_once.swap(false, Ordering::SeqCst)
        {
            self.failures_exhausted.add_permits(1);
            self.allow_terminal_commit.acquire().await.unwrap().forget();
        }
        let result = self.inner.apply(revision, changes, events).await;
        if result.is_ok()
            && matches!(
                status.as_deref(),
                Some("completed" | "failed" | "cancelled" | "limit_exceeded")
            )
        {
            self.terminal_committed.add_permits(1);
        }
        result
    }

    async fn replay(
        &self,
        after: u64,
        limit: usize,
    ) -> Result<Vec<DurableEvent>, ControlStoreError> {
        self.inner.replay(after, limit).await
    }

    async fn event_bounds(&self) -> Result<ait_ports::EventBounds, ControlStoreError> {
        self.inner.event_bounds().await
    }

    async fn replay_page(
        &self,
        after: u64,
        limit: usize,
    ) -> Result<ait_ports::DurableEventPage, ControlStoreError> {
        self.inner.replay_page(after, limit).await
    }

    async fn save_progress(
        &self,
        checkpoint: ait_ports::ProgressCheckpoint,
        events: Vec<PendingEvent>,
    ) -> Result<(), ControlStoreError> {
        self.inner.save_progress(checkpoint, events).await
    }

    async fn load_progress(&self) -> Result<Vec<ait_ports::ProgressCheckpoint>, ControlStoreError> {
        self.inner.load_progress().await
    }

    async fn clear_progress(&self, run_id: &str) -> Result<(), ControlStoreError> {
        self.inner.clear_progress(run_id).await
    }
}

#[tokio::test]
async fn active_session_rejects_competitors_and_same_project_writers_are_serialized() {
    let store = Arc::new(SqliteControlStore::in_memory().unwrap());
    let agent = Arc::new(BlockingAgent::new());
    let service = Arc::new(LocalControlService::with_workspace_agent(
        store,
        agent.clone(),
    ));
    let directory = setup(&service, config("high")).await;
    let running = {
        let service = service.clone();
        tokio::spawn(async move { ok(&service, send("one")).await })
    };
    agent.started().await;
    let before = view(&service).await;
    // Busy admission precedes even the clean-Git prerequisite.
    std::fs::write(directory.path().join("dirty"), "temporary").unwrap();
    for command in [
        send("one"),
        Command::SetSessionConfig {
            session_id: "one".into(),
            config: config("low"),
        },
        Command::SetSessionAgent {
            session_id: "one".into(),
            agent_id: "preset".into(),
        },
    ] {
        let response = tokio::time::timeout(Duration::from_millis(500), service.execute(command))
            .await
            .unwrap();
        assert_eq!(response.error.unwrap().code, ErrorCode::SessionBusy);
    }
    assert_eq!(view(&service).await, before);
    std::fs::remove_file(directory.path().join("dirty")).unwrap();
    // Changing a shared preset cannot alter the already pinned Run.
    ok(
        &service,
        Command::UpdateAgent {
            id: "preset".into(),
            name: "Shared".into(),
            config: config("low"),
        },
    )
    .await;
    let second = {
        let service = service.clone();
        tokio::spawn(async move { ok(&service, send("two")).await })
    };
    assert!(
        tokio::time::timeout(Duration::from_millis(100), agent.entered.acquire())
            .await
            .is_err(),
        "the second Session must wait for the same Project write lease"
    );
    agent.release.add_permits(1);
    let CommandResult::Run(first_run) = running.await.unwrap() else {
        panic!()
    };
    assert_eq!(first_run.status, "completed");
    agent.started().await;
    assert_eq!(view(&service).await.runs.len(), 2);
    assert_eq!(
        *agent.requests.lock().unwrap(),
        vec![
            ("gpt-5.6-sol".into(), Some("high".into())),
            ("gpt-5.6-sol".into(), Some("low".into()))
        ]
    );
    agent.release.add_permits(1);
    let CommandResult::Run(second_run) = second.await.unwrap() else {
        panic!()
    };
    assert_eq!(second_run.status, "completed");
    let finished = view(&service).await;
    assert!(
        finished
            .sessions
            .iter()
            .all(|session| session.active_run_id.is_none())
    );
    assert_eq!(
        finished
            .runs
            .iter()
            .find(|run| run.id == first_run.id)
            .unwrap()
            .config,
        config("high")
    );
    assert_eq!(
        finished
            .runs
            .iter()
            .find(|run| run.id == second_run.id)
            .unwrap()
            .config,
        config("low")
    );
    assert_eq!(
        finished
            .messages
            .iter()
            .filter(|m| m.role == "user")
            .count(),
        2
    );
}

#[tokio::test]
async fn codex_writers_for_unrelated_projects_enter_concurrently() {
    let store = Arc::new(SqliteControlStore::in_memory().unwrap());
    let agent = Arc::new(BlockingAgent::new());
    let service = Arc::new(LocalControlService::with_workspace_agent(
        store,
        agent.clone(),
    ));
    let first = tempfile::tempdir().unwrap();
    let second = tempfile::tempdir().unwrap();
    for (project_id, directory) in [("p1", &first), ("p2", &second)] {
        ok(
            &service,
            Command::RegisterProject {
                id: project_id.into(),
                name: project_id.into(),
                workdir: directory.path().display().to_string(),
                repo_url: None,
            },
        )
        .await;
    }
    ok(
        &service,
        Command::RegisterAgent {
            id: "preset".into(),
            name: "Shared".into(),
            config: config("high"),
        },
    )
    .await;
    for (session_id, project_id) in [("one", "p1"), ("two", "p2")] {
        ok(
            &service,
            Command::CreateSession {
                id: session_id.into(),
                project_id: project_id.into(),
                agent_id: "preset".into(),
                at_message_id: None,
            },
        )
        .await;
    }

    let one = {
        let service = service.clone();
        tokio::spawn(async move { ok(&service, send("one")).await })
    };
    let two = {
        let service = service.clone();
        tokio::spawn(async move { ok(&service, send("two")).await })
    };
    agent.started().await;
    agent.started().await;
    assert_eq!(agent.requests.lock().unwrap().len(), 2);
    agent.release.add_permits(2);
    for task in [one, two] {
        let CommandResult::Run(run) = task.await.unwrap() else {
            panic!()
        };
        assert_eq!(run.status, "completed");
    }
}

#[tokio::test]
async fn a_second_service_cannot_bypass_the_process_wide_workspace_lease() {
    let store = Arc::new(SqliteControlStore::in_memory().unwrap());
    let agent = Arc::new(BlockingAgent::new());
    let first_service = Arc::new(LocalControlService::with_workspace_agent(
        store.clone(),
        agent.clone(),
    ));
    let second_service = LocalControlService::with_workspace_agent(store, agent.clone());
    let _directory = setup(&first_service, config("high")).await;
    let running = {
        let service = first_service.clone();
        tokio::spawn(async move { ok(&service, send("one")).await })
    };
    agent.started().await;

    let rejected = second_service.execute(send("two")).await;
    assert_eq!(
        rejected.error.unwrap().code,
        ErrorCode::ProjectWorkspaceBusy
    );
    assert_eq!(view(&first_service).await.runs.len(), 1);

    agent.release.add_permits(1);
    let CommandResult::Run(finished) = running.await.unwrap() else {
        panic!()
    };
    assert_eq!(finished.status, "completed");
    let retried = {
        let service = Arc::new(second_service);
        let task_service = service.clone();
        let task = tokio::spawn(async move { ok(&task_service, send("two")).await });
        agent.started().await;
        agent.release.add_permits(1);
        task.await.unwrap()
    };
    let CommandResult::Run(retried) = retried else {
        panic!()
    };
    assert_eq!(retried.status, "completed");
}

#[cfg(unix)]
#[tokio::test]
async fn canonical_path_aliases_share_the_same_process_wide_lease() {
    let root = tempfile::tempdir().unwrap();
    let project = root.path().join("project");
    let alias = root.path().join("project-alias");
    std::fs::create_dir(&project).unwrap();
    std::os::unix::fs::symlink(&project, &alias).unwrap();
    let agent = Arc::new(BlockingAgent::new());
    let first_service = Arc::new(LocalControlService::with_workspace_agent(
        Arc::new(SqliteControlStore::in_memory().unwrap()),
        agent.clone(),
    ));
    let second_service = LocalControlService::with_workspace_agent(
        Arc::new(SqliteControlStore::in_memory().unwrap()),
        agent.clone(),
    );
    for (service, workdir, session_id) in [
        (first_service.as_ref(), project.as_path(), "one"),
        (&second_service, alias.as_path(), "two"),
    ] {
        ok(
            service,
            Command::RegisterProject {
                id: "p".into(),
                name: "Project".into(),
                workdir: workdir.display().to_string(),
                repo_url: None,
            },
        )
        .await;
        ok(
            service,
            Command::RegisterAgent {
                id: "preset".into(),
                name: "Shared".into(),
                config: config("high"),
            },
        )
        .await;
        ok(
            service,
            Command::CreateSession {
                id: session_id.into(),
                project_id: "p".into(),
                agent_id: "preset".into(),
                at_message_id: None,
            },
        )
        .await;
    }
    let running = {
        let service = first_service.clone();
        tokio::spawn(async move { ok(&service, send("one")).await })
    };
    agent.started().await;

    let rejected = second_service.execute(send("two")).await;
    assert_eq!(
        rejected.error.unwrap().code,
        ErrorCode::ProjectWorkspaceBusy
    );
    agent.release.add_permits(1);
    let CommandResult::Run(finished) = running.await.unwrap() else {
        panic!()
    };
    assert_eq!(finished.status, "completed");
}

#[tokio::test]
async fn serialized_sessions_capture_new_baselines_and_own_only_their_commits() {
    let store = Arc::new(SqliteControlStore::in_memory().unwrap());
    let agent = Arc::new(CommittingAgent::new());
    let service = Arc::new(LocalControlService::with_workspace_agent(
        store,
        agent.clone(),
    ));
    let directory = setup(&service, config("high")).await;
    let initial = git_head(directory.path());
    let initial_tree = git_index_tree(directory.path());

    let first = {
        let service = service.clone();
        tokio::spawn(async move { ok(&service, send_text("one", "first change")).await })
    };
    agent.started().await;
    let second = {
        let service = service.clone();
        tokio::spawn(async move { ok(&service, send_text("two", "second change")).await })
    };
    assert!(
        tokio::time::timeout(Duration::from_millis(100), agent.entered.acquire())
            .await
            .is_err()
    );
    agent.release.add_permits(1);
    let CommandResult::Run(first_run) = first.await.unwrap() else {
        panic!()
    };
    agent.started().await;
    agent.release.add_permits(1);
    let CommandResult::Run(second_run) = second.await.unwrap() else {
        panic!()
    };

    let commits = agent.commits.lock().unwrap().clone();
    assert_eq!(commits.len(), 2);
    assert_eq!(commits[0].0, initial);
    assert_eq!(commits[1].0, commits[0].1);
    assert_eq!(first_run.status, "completed");
    assert_eq!(second_run.status, "completed");
    assert_eq!(
        first_run.workspace_base_commit.as_deref(),
        Some(initial.as_str())
    );
    assert_eq!(
        first_run.workspace_base_index_tree.as_deref(),
        Some(initial_tree.as_str())
    );
    assert_eq!(
        second_run.workspace_base_commit.as_deref(),
        Some(commits[0].1.as_str())
    );
    assert_eq!(
        second_run.workspace_base_index_tree.as_deref(),
        Some(git_commit_tree(directory.path(), &commits[0].1).as_str())
    );
    assert_ne!(first_run.id, second_run.id);
    for (baseline, commit, file) in &commits {
        assert_ne!(baseline, commit);
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(directory.path())
            .args(["show", "--pretty=format:", "--name-only", commit])
            .output()
            .unwrap();
        assert_eq!(String::from_utf8(output.stdout).unwrap().trim(), file);
    }
    let workspace = view(&service).await;
    let message_commit = |id: &str| {
        workspace
            .messages
            .iter()
            .find(|message| message.id == id)
            .unwrap()
            .git_commit
            .clone()
            .unwrap()
    };
    assert_eq!(message_commit(&first_run.base_message_id), initial);
    assert_eq!(message_commit(&second_run.base_message_id), commits[0].1);
    assert_eq!(git_head(directory.path()), commits[1].1);
}

#[tokio::test]
async fn cancellation_holds_the_workspace_lease_until_adapter_settlement() {
    let store = Arc::new(SqliteControlStore::in_memory().unwrap());
    let agent = Arc::new(SettlingCancellationAgent::new());
    let service = Arc::new(LocalControlService::with_workspace_agent(
        store,
        agent.clone(),
    ));
    let _directory = setup(&service, config("high")).await;
    let first = {
        let service = service.clone();
        tokio::spawn(async move { ok(&service, send("one")).await })
    };
    agent.started().await;
    let active = view(&service).await.runs[0].id.clone();
    ok(
        &service,
        Command::CancelRun {
            run_id: active.clone(),
        },
    )
    .await;
    agent.cancelling.acquire().await.unwrap().forget();

    let second = {
        let service = service.clone();
        tokio::spawn(async move { ok(&service, send("two")).await })
    };
    assert!(
        tokio::time::timeout(Duration::from_millis(100), agent.entered.acquire())
            .await
            .is_err(),
        "the next Run entered before cancelled workspace settlement completed"
    );
    agent.settle_release.add_permits(1);
    let CommandResult::Run(cancelled) = first.await.unwrap() else {
        panic!()
    };
    assert_eq!(cancelled.id, active);
    assert_eq!(cancelled.status, "cancelled");
    agent.started().await;
    let CommandResult::Run(completed) = second.await.unwrap() else {
        panic!()
    };
    assert_eq!(completed.status, "completed");
}

#[tokio::test]
async fn dropping_the_execute_future_keeps_leases_and_terminal_persistence_supervised() {
    let store = Arc::new(SqliteControlStore::in_memory().unwrap());
    let agent = Arc::new(BlockingAgent::new());
    let service = Arc::new(LocalControlService::with_workspace_agent(
        store,
        agent.clone(),
    ));
    let _directory = setup(&service, config("high")).await;
    let abandoned_transport = {
        let service = service.clone();
        tokio::spawn(async move { service.execute(send("one")).await })
    };
    agent.started().await;
    abandoned_transport.abort();
    assert!(abandoned_transport.await.unwrap_err().is_cancelled());

    let second = {
        let service = service.clone();
        tokio::spawn(async move { ok(&service, send("two")).await })
    };
    assert!(
        tokio::time::timeout(Duration::from_millis(100), agent.entered.acquire())
            .await
            .is_err(),
        "the detached transport released the Project lease before settlement"
    );

    agent.release.add_permits(1);
    agent.started().await;
    let settled = view(&service).await;
    let first_run = settled
        .runs
        .iter()
        .find(|run| run.session_id.as_deref() == Some("one"))
        .unwrap();
    assert_eq!(first_run.status, "completed");
    assert!(
        settled
            .sessions
            .iter()
            .find(|session| session.id == "one")
            .unwrap()
            .active_run_id
            .is_none()
    );

    agent.release.add_permits(1);
    let CommandResult::Run(second_run) = second.await.unwrap() else {
        panic!()
    };
    assert_eq!(second_run.status, "completed");
}

#[tokio::test]
async fn cancellation_wins_the_finalization_gate_before_integration() {
    let store = Arc::new(SqliteControlStore::in_memory().unwrap());
    let agent = Arc::new(FinalizationRaceAgent::new());
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
    let active = view(&service).await.runs[0].id.clone();

    let CommandResult::Run(cancelled) = ok(
        &service,
        Command::CancelRun {
            run_id: active.clone(),
        },
    )
    .await
    else {
        panic!()
    };
    assert_eq!(cancelled.status, "cancelled");
    agent.begin_integration.add_permits(1);

    let CommandResult::Run(settled) = running.await.unwrap() else {
        panic!()
    };
    assert_eq!(settled.status, "cancelled");
    assert!(
        tokio::time::timeout(
            Duration::from_millis(100),
            agent.integration_started.acquire()
        )
        .await
        .is_err(),
        "integration started after durable cancellation"
    );
    let workspace = view(&service).await;
    assert_eq!(workspace.messages.len(), 2);
    assert!(
        workspace
            .sessions
            .iter()
            .find(|session| session.id == "one")
            .unwrap()
            .active_run_id
            .is_none()
    );
}

#[tokio::test]
async fn integration_wins_the_finalization_gate_and_its_output_is_persisted() {
    let store = Arc::new(SqliteControlStore::in_memory().unwrap());
    let agent = Arc::new(FinalizationRaceAgent::new());
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
    let active = view(&service).await.runs[0].id.clone();
    agent.begin_integration.add_permits(1);
    agent.integration_started.acquire().await.unwrap().forget();

    let rejected = service
        .execute(Command::CancelRun {
            run_id: active.clone(),
        })
        .await;
    assert_eq!(rejected.error.unwrap().code, ErrorCode::RunAlreadyTerminal);
    assert_eq!(
        view(&service)
            .await
            .runs
            .into_iter()
            .find(|run| run.id == active)
            .unwrap()
            .status,
        "settling"
    );

    agent.complete.add_permits(1);
    let CommandResult::Run(completed) = running.await.unwrap() else {
        panic!()
    };
    assert_eq!(completed.status, "completed");
    let workspace = view(&service).await;
    let assistant = workspace
        .messages
        .iter()
        .find(|message| message.role == "assistant")
        .unwrap();
    assert_eq!(assistant.text.as_deref(), Some("integrated result"));
    assert_eq!(
        assistant.data.as_ref().unwrap()["codex"]["commit_id"],
        serde_json::json!("fixture-commit")
    );
}

#[tokio::test]
async fn integration_keeps_finalization_and_workspace_admission_until_terminal_store_recovers() {
    let failures = VecDeque::from([
        ControlStoreError::Conflict,
        ControlStoreError::Conflict,
        ControlStoreError::Conflict,
        ControlStoreError::Conflict,
        ControlStoreError::Conflict,
        ControlStoreError::Other("injected terminal store failure".into()),
    ]);
    let store = Arc::new(TerminalFailingStore {
        inner: SqliteControlStore::in_memory().unwrap(),
        failure_status: "completed",
        pause_status: "completed",
        failures: Mutex::new(failures),
        pause_once: AtomicBool::new(true),
        failures_exhausted: Semaphore::new(0),
        allow_terminal_commit: Semaphore::new(0),
        terminal_committed: Semaphore::new(0),
    });
    let agent = Arc::new(FinalizationRaceAgent::new());
    let service = Arc::new(LocalControlService::with_workspace_agent(
        store.clone(),
        agent.clone(),
    ));
    let _directory = setup(&service, config("high")).await;
    let first = {
        let service = service.clone();
        tokio::spawn(async move { ok(&service, send("one")).await })
    };
    agent.started().await;
    let active = view(&service).await.runs[0].id.clone();
    agent.begin_integration.add_permits(1);
    agent.integration_started.acquire().await.unwrap().forget();
    agent.complete.add_permits(1);
    tokio::time::timeout(Duration::from_secs(3), store.failures_exhausted.acquire())
        .await
        .unwrap()
        .unwrap()
        .forget();

    assert!(store.failures.lock().unwrap().is_empty());
    assert_eq!(
        view(&service)
            .await
            .runs
            .into_iter()
            .find(|run| run.id == active)
            .unwrap()
            .status,
        "settling"
    );
    let rejected = service
        .execute(Command::CancelRun {
            run_id: active.clone(),
        })
        .await;
    assert_eq!(rejected.error.unwrap().code, ErrorCode::RunAlreadyTerminal);

    let second = {
        let service = service.clone();
        tokio::spawn(async move { ok(&service, send("two")).await })
    };
    assert!(
        tokio::time::timeout(Duration::from_millis(100), agent.entered.acquire())
            .await
            .is_err(),
        "a new same-Project Run entered while terminal persistence was unavailable"
    );

    store.allow_terminal_commit.add_permits(1);
    let CommandResult::Run(completed) = first.await.unwrap() else {
        panic!()
    };
    assert_eq!(completed.id, active);
    assert_eq!(completed.status, "completed");
    agent.started().await;
    agent.begin_integration.add_permits(1);
    agent.integration_started.acquire().await.unwrap().forget();
    agent.complete.add_permits(1);
    let CommandResult::Run(second) = second.await.unwrap() else {
        panic!()
    };
    assert_eq!(second.status, "completed");

    let workspace = view(&service).await;
    let first = workspace.runs.iter().find(|run| run.id == active).unwrap();
    assert_eq!(first.status, "completed");
    let assistant = workspace
        .messages
        .iter()
        .find(|message| Some(&message.id) == first.last_message_id.as_ref())
        .unwrap();
    assert_eq!(assistant.text.as_deref(), Some("integrated result"));
    assert_eq!(
        assistant.data.as_ref().unwrap()["codex"]["commit_id"],
        serde_json::json!("fixture-commit")
    );
}

async fn verify_running_transition_failure_reaches_terminal(
    failures: VecDeque<ControlStoreError>,
    expected_code: ErrorCode,
) {
    let store = Arc::new(TerminalFailingStore {
        inner: SqliteControlStore::in_memory().unwrap(),
        failure_status: "running",
        pause_status: "failed",
        failures: Mutex::new(failures),
        pause_once: AtomicBool::new(true),
        failures_exhausted: Semaphore::new(0),
        allow_terminal_commit: Semaphore::new(0),
        terminal_committed: Semaphore::new(0),
    });
    let agent = Arc::new(BlockingAgent::new());
    let service = Arc::new(LocalControlService::with_workspace_agent(
        store.clone(),
        agent.clone(),
    ));
    let _directory = setup(&service, config("high")).await;

    let accepted = submit_run(&service, send("one")).await;
    assert_eq!(accepted.status, "queued");
    wait_for_signal(&store.failures_exhausted).await;
    let pending = view(&service).await;
    assert_eq!(
        pending
            .runs
            .iter()
            .find(|run| run.id == accepted.id)
            .unwrap()
            .status,
        "queued"
    );
    assert_eq!(
        pending
            .sessions
            .iter()
            .find(|session| session.id == "one")
            .unwrap()
            .active_run_id
            .as_deref(),
        Some(accepted.id.as_str())
    );

    let second = {
        let service = service.clone();
        tokio::spawn(async move { submit_run(&service, send("two")).await })
    };
    assert!(
        tokio::time::timeout(Duration::from_millis(100), agent.entered.acquire())
            .await
            .is_err(),
        "same-Project admission escaped before the first Run was terminal"
    );

    store.allow_terminal_commit.add_permits(1);
    wait_for_signal(&store.terminal_committed).await;
    let failed = view(&service).await;
    let first = failed
        .runs
        .iter()
        .find(|run| run.id == accepted.id)
        .unwrap();
    assert_eq!(first.status, "failed");
    assert_eq!(first.error.as_ref().unwrap().code, expected_code);
    assert!(
        failed
            .sessions
            .iter()
            .find(|session| session.id == "one")
            .unwrap()
            .active_run_id
            .is_none()
    );

    let second = second.await.unwrap();
    agent.started().await;
    agent.release.add_permits(1);
    wait_for_signal(&store.terminal_committed).await;
    assert_eq!(
        view(&service)
            .await
            .runs
            .iter()
            .find(|run| run.id == second.id)
            .unwrap()
            .status,
        "completed"
    );
}

#[tokio::test]
async fn admitted_run_reaches_terminal_after_running_transition_conflict_or_store_error() {
    verify_running_transition_failure_reaches_terminal(
        VecDeque::from([
            ControlStoreError::Conflict,
            ControlStoreError::Conflict,
            ControlStoreError::Conflict,
            ControlStoreError::Conflict,
        ]),
        ErrorCode::RunQueueConflict,
    )
    .await;
    verify_running_transition_failure_reaches_terminal(
        VecDeque::from([ControlStoreError::Other(
            "injected running transition failure".into(),
        )]),
        ErrorCode::RunRecoveryFailed,
    )
    .await;
}

async fn verify_result_checkpoint_failure_never_claims_integration(
    failures: VecDeque<ControlStoreError>,
    expected_code: ErrorCode,
) {
    let store = Arc::new(TerminalFailingStore {
        inner: SqliteControlStore::in_memory().unwrap(),
        failure_status: "settling",
        pause_status: "failed",
        failures: Mutex::new(failures),
        pause_once: AtomicBool::new(false),
        failures_exhausted: Semaphore::new(0),
        allow_terminal_commit: Semaphore::new(0),
        terminal_committed: Semaphore::new(0),
    });
    let agent = Arc::new(FinalizationRaceAgent::new());
    let service = Arc::new(LocalControlService::with_workspace_agent(
        store.clone(),
        agent.clone(),
    ));
    let _directory = setup(&service, config("high")).await;

    let accepted = submit_run(&service, send("one")).await;
    agent.started().await;
    agent.begin_integration.add_permits(1);
    wait_for_signal(&store.terminal_committed).await;
    assert!(
        tokio::time::timeout(
            Duration::from_millis(100),
            agent.integration_started.acquire()
        )
        .await
        .is_err(),
        "integration was claimed without a durable result checkpoint"
    );
    let workspace = view(&service).await;
    let failed = workspace
        .runs
        .iter()
        .find(|run| run.id == accepted.id)
        .unwrap();
    assert_eq!(failed.status, "failed");
    assert_eq!(failed.error.as_ref().unwrap().code, expected_code);
    assert!(
        workspace
            .messages
            .iter()
            .all(|message| message.role != "assistant")
    );
}

#[tokio::test]
async fn result_checkpoint_failure_never_claims_integration_or_completes() {
    verify_result_checkpoint_failure_never_claims_integration(
        VecDeque::from([
            ControlStoreError::Conflict,
            ControlStoreError::Conflict,
            ControlStoreError::Conflict,
            ControlStoreError::Conflict,
        ]),
        ErrorCode::RunQueueConflict,
    )
    .await;
    verify_result_checkpoint_failure_never_claims_integration(
        VecDeque::from([ControlStoreError::Other(
            "injected settling transition failure".into(),
        )]),
        ErrorCode::RunRecoveryFailed,
    )
    .await;
}

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

#[derive(Default)]
struct Gateway {
    omit_models: std::sync::atomic::AtomicBool,
    secrets: Mutex<HashMap<String, String>>,
    calls: Mutex<Vec<(AgentConfiguration, Vec<String>)>>,
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

#[derive(Default)]
struct CapturingWorkspaceAgent(Mutex<Vec<WorkspaceAgentInvocation>>);

#[async_trait]
impl WorkspaceAgent for CapturingWorkspaceAgent {
    async fn invoke(
        &self,
        request: WorkspaceAgentInvocation,
    ) -> Result<WorkspaceAgentResponse, DomainError> {
        self.0.lock().unwrap().push(request);
        Ok(WorkspaceAgentResponse {
            assistant_text: "native result".into(),
            commit_id: None,
            operations: Vec::new(),
            output_items: Vec::new(),
        })
    }
}

async fn save_permission_settings(service: &LocalControlService, sandbox: &str, approval: &str) {
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

#[tokio::test]
async fn codex_permission_settings_are_snapshotted_into_each_run_and_native_invocation() {
    for (setting, expected) in [
        ("read_only", SandboxAccess::ReadOnly),
        ("strict", SandboxAccess::ReadOnly),
        ("workspace_write", SandboxAccess::WorkspaceWrite),
        ("full_access", SandboxAccess::FullAccess),
    ] {
        let store = Arc::new(SqliteControlStore::in_memory().unwrap());
        let native = Arc::new(CapturingWorkspaceAgent::default());
        let service = LocalControlService::with_workspace_agent(store, native.clone());
        let _directory = setup(&service, config("high")).await;
        save_permission_settings(&service, setting, "untrusted_only").await;
        let CommandResult::Run(run) = ok(&service, send("one")).await else {
            panic!("expected Run")
        };
        let expected_profile = RunPermissionProfile {
            sandbox: expected,
            approval: ApprovalMode::UntrustedOnly,
        };
        assert_eq!(run.permission_profile, expected_profile);
        assert_eq!(
            native.0.lock().unwrap()[0].permission_profile,
            expected_profile
        );

        let mut changed = default_settings();
        changed
            .0
            .insert("permissions.sandbox".into(), serde_json::json!("read_only"));
        let _ = ok(
            &service,
            Command::SaveSettings {
                expected_revision: 2,
                values: changed,
            },
        )
        .await;
        let persisted = view(&service)
            .await
            .runs
            .into_iter()
            .find(|candidate| candidate.id == run.id)
            .unwrap();
        assert_eq!(persisted.permission_profile, expected_profile);
    }
}

#[tokio::test]
async fn fresh_and_reset_settings_are_read_only_while_explicit_write_survives_restart() {
    let store = Arc::new(SqliteControlStore::in_memory().unwrap());
    let native = Arc::new(CapturingWorkspaceAgent::default());
    let service = LocalControlService::with_workspace_agent(store.clone(), native.clone());
    let _directory = setup(&service, config("high")).await;
    let CommandResult::Run(fresh) = ok(&service, send("one")).await else {
        panic!("expected Run")
    };
    assert_eq!(fresh.permission_profile.sandbox, SandboxAccess::ReadOnly);
    save_permission_settings(&service, "workspace_write", "on_request").await;
    drop(service);

    let restarted = LocalControlService::with_workspace_agent(store, native.clone());
    let CommandResult::Run(explicit) = ok(&restarted, send("two")).await else {
        panic!("expected Run")
    };
    assert_eq!(
        explicit.permission_profile.sandbox,
        SandboxAccess::WorkspaceWrite
    );
    let _ = ok(&restarted, Command::ResetSettings).await;
    let CommandResult::Run(reset) = ok(&restarted, send("one")).await else {
        panic!("expected Run")
    };
    assert_eq!(reset.permission_profile.sandbox, SandboxAccess::ReadOnly);
    assert_eq!(
        native
            .0
            .lock()
            .unwrap()
            .iter()
            .map(|call| call.permission_profile.sandbox)
            .collect::<Vec<_>>(),
        vec![
            SandboxAccess::ReadOnly,
            SandboxAccess::WorkspaceWrite,
            SandboxAccess::ReadOnly,
        ]
    );
}

#[tokio::test]
async fn read_only_protocol_write_attempt_fails_run_without_project_or_message_side_effects() {
    let store = Arc::new(SqliteControlStore::in_memory().unwrap());
    let workspace_agent = Arc::new(CodexWorkspaceAgent::new(Arc::new(ReadOnlyViolatingAdapter)));
    let service = LocalControlService::with_workspace_agent(store, workspace_agent);
    let directory = setup(&service, config("high")).await;
    let baseline = git_head(directory.path());
    let baseline_index = git_index_tree(directory.path());
    let CommandResult::Run(run) = ok(&service, send("one")).await else {
        panic!("expected Run")
    };

    assert_eq!(run.permission_profile.sandbox, SandboxAccess::ReadOnly);
    assert_eq!(run.status, "failed");
    assert_eq!(run.error.unwrap().code, ErrorCode::ProjectGitDirty);
    assert_eq!(git_head(directory.path()), baseline);
    assert_eq!(git_index_tree(directory.path()), baseline_index);
    assert!(!directory.path().join("unauthorized.txt").exists());
    let snapshot = view(&service).await;
    assert!(
        snapshot
            .messages
            .iter()
            .all(|message| message.role != "assistant")
    );
    assert_eq!(
        snapshot
            .messages
            .iter()
            .filter(|message| message.role == "user")
            .count(),
        1
    );
}

#[tokio::test]
async fn unsupported_or_administrator_conflicting_policies_fail_before_messages_or_agents() {
    let store = Arc::new(SqliteControlStore::in_memory().unwrap());
    let native = Arc::new(CapturingWorkspaceAgent::default());
    let service = LocalControlService::with_workspace_agent(store.clone(), native.clone())
        .with_permission_limits(PermissionPolicyLimits {
            max_sandbox: SandboxAccess::ReadOnly,
            allow_session_approvals: true,
        });
    let _directory = setup(&service, config("high")).await;
    save_permission_settings(&service, "full_access", "on_request").await;
    let rejected = service.execute(send("one")).await;
    assert_eq!(
        rejected.error.unwrap().code,
        ErrorCode::InvalidConfiguration
    );
    assert_eq!(view(&service).await.messages.len(), 1);
    assert!(native.0.lock().unwrap().is_empty());

    let mut always = default_settings();
    always
        .0
        .insert("permissions.sandbox".into(), serde_json::json!("read_only"));
    always
        .0
        .insert("permissions.approval".into(), serde_json::json!("always"));
    let _ = ok(
        &service,
        Command::SaveSettings {
            expected_revision: 2,
            values: always,
        },
    )
    .await;
    let rejected = service.execute(send("two")).await;
    assert_eq!(
        rejected.error.unwrap().code,
        ErrorCode::InvalidConfiguration
    );
    assert_eq!(view(&service).await.messages.len(), 1);
    assert!(native.0.lock().unwrap().is_empty());

    let snapshot = store.load().await.unwrap();
    let mut corrupted = snapshot.value;
    corrupted["settings"]["permissions.sandbox"] = serde_json::json!("unknown-policy");
    store
        .replace_state(snapshot.revision, corrupted, Vec::new())
        .await
        .unwrap();
    let rejected = service.execute(send("two")).await;
    assert_eq!(
        rejected.error.unwrap().code,
        ErrorCode::InvalidConfiguration
    );
    assert_eq!(view(&service).await.messages.len(), 1);
    assert!(native.0.lock().unwrap().is_empty());
}

struct ApprovalAgent {
    kind: NativeApprovalKind,
    permission_path: Option<String>,
    requested: Semaphore,
    decision: Mutex<Option<WorkspaceApprovalDecision>>,
}

impl ApprovalAgent {
    fn new(kind: NativeApprovalKind) -> Self {
        Self {
            kind,
            permission_path: None,
            requested: Semaphore::new(0),
            decision: Mutex::new(None),
        }
    }

    fn permissions_at(path: impl Into<String>) -> Self {
        Self {
            kind: NativeApprovalKind::Permissions,
            permission_path: Some(path.into()),
            requested: Semaphore::new(0),
            decision: Mutex::new(None),
        }
    }
}

#[async_trait]
impl WorkspaceAgent for ApprovalAgent {
    async fn invoke(
        &self,
        request: WorkspaceAgentInvocation,
    ) -> Result<WorkspaceAgentResponse, DomainError> {
        self.requested.add_permits(1);
        let permission_path = self
            .permission_path
            .clone()
            .unwrap_or_else(|| request.cwd.to_string_lossy().into_owned());
        let decision = request
            .approvals
            .decide(WorkspaceApprovalRequest {
                run_id: request.request_id.clone(),
                protocol_request_id: serde_json::json!(73),
                method: match self.kind {
                    NativeApprovalKind::Permissions => "item/permissions/requestApproval",
                    NativeApprovalKind::FileChange => "item/fileChange/requestApproval",
                    _ => "item/commandExecution/requestApproval",
                }
                .into(),
                kind: self.kind,
                thread_id: "thread-a".into(),
                turn_id: "turn-a".into(),
                item_id: "item-a".into(),
                target: match self.kind {
                    NativeApprovalKind::Permissions => NativeApprovalTarget::Permissions {
                        cwd: request.cwd.to_string_lossy().into_owned(),
                    },
                    NativeApprovalKind::FileChange => NativeApprovalTarget::FileChange {
                        grant_root: Some(request.cwd.to_string_lossy().into_owned()),
                        changes: Vec::new(),
                    },
                    _ => NativeApprovalTarget::Command {
                        command: "git status".into(),
                        cwd: request.cwd.to_string_lossy().into_owned(),
                    },
                },
                requested_permissions: (self.kind == NativeApprovalKind::Permissions).then(|| {
                    serde_json::json!({
                        "fileSystem": {"write": [permission_path]}
                    })
                }),
            })
            .await?;
        *self.decision.lock().unwrap() = Some(decision);
        Ok(WorkspaceAgentResponse {
            assistant_text: "approval settled".into(),
            commit_id: None,
            operations: Vec::new(),
            output_items: Vec::new(),
        })
    }
}

#[tokio::test]
async fn native_approval_wait_is_nonblocking_durable_and_duplicate_safe() {
    let store = Arc::new(SqliteControlStore::in_memory().unwrap());
    let agent = Arc::new(ApprovalAgent::new(NativeApprovalKind::CommandExecution));
    let service = Arc::new(LocalControlService::with_workspace_agent(
        store,
        agent.clone(),
    ));
    let _directory = setup(&service, config("high")).await;
    let running = {
        let service = service.clone();
        tokio::spawn(async move { service.execute(send("one")).await })
    };
    wait_for_signal(&agent.requested).await;

    let pending = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let runs = view(&service).await.runs;
            if let Some(approval) = runs
                .iter()
                .flat_map(|run| &run.native_approvals)
                .find(|approval| approval.status == ait_domain::NativeApprovalStatus::Pending)
            {
                break (approval.run_id.clone(), approval.id.clone());
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    // Reads and event dispatch remain live while the provider task awaits a member decision.
    tokio::time::timeout(Duration::from_millis(500), view(&service))
        .await
        .unwrap();
    let resolved = service
        .execute(Command::ResolveNativeApproval {
            run_id: pending.0.clone(),
            approval_id: pending.1.clone(),
            action: NativeApprovalAction::Approve,
            scope: Some(ApprovalGrantScope::OneShot),
        })
        .await;
    assert!(resolved.ok, "{:?}", resolved.error);
    let duplicate = service
        .execute(Command::ResolveNativeApproval {
            run_id: pending.0,
            approval_id: pending.1,
            action: NativeApprovalAction::Deny,
            scope: None,
        })
        .await;
    assert_eq!(
        duplicate.error.unwrap().code,
        ErrorCode::ToolApprovalRequired
    );
    assert!(running.await.unwrap().ok);
    assert_eq!(
        *agent.decision.lock().unwrap(),
        Some(WorkspaceApprovalDecision::Approved {
            scope: ApprovalGrantScope::OneShot,
            permissions: None,
        })
    );
}

#[cfg(unix)]
#[tokio::test]
async fn command_approval_secrets_never_reach_durable_or_reconnected_views() {
    use ait_agent_adapters::codex::CodexAppServerConfig;
    use ait_contracts::ProtocolRequestId;
    use std::os::unix::fs::PermissionsExt as _;

    let fake_server = tempfile::tempdir().unwrap();
    let binary = fake_server.path().join("fake-codex");
    std::fs::write(
        &binary,
        r#"#!/bin/sh
IFS= read -r _ || exit 60
printf '%s\n' '{"id":0,"result":{}}'
IFS= read -r _ || exit 61
IFS= read -r _ || exit 62
printf '%s\n' '{"id":1,"result":{"thread":{"id":"thread-a"}}}'
IFS= read -r _ || exit 63
printf '%s\n' '{"id":2,"result":{"turn":{"id":"turn-a"}}}'
printf '%s\n' '{"id":73,"method":"item/commandExecution/requestApproval","params":{"threadId":"thread-a","turnId":"turn-a","itemId":"command-a","command":"curl -H X-Api-Key:header-secret --header=\"Authorization: Bearer auth-secret\" -H \"Cookie: session=cookie-secret\" https://url-user:url-secret@example.test/v1","cwd":"/workspace","reason":"offline fixture"}}'
IFS= read -r approval_response || exit 64
case "$approval_response" in
  *'"id":73'*'"decision":"accept"'*) ;;
  *) exit 65 ;;
esac
printf '%s\n' '{"method":"item/agentMessage/delta","params":{"threadId":"thread-a","turnId":"turn-a","itemId":"answer-a","delta":"approved"}}'
printf '%s\n' '{"method":"turn/completed","params":{"threadId":"thread-a","turn":{"id":"turn-a","items":[],"status":"completed"}}}'
"#,
    )
    .unwrap();
    std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700)).unwrap();

    let store = Arc::new(SqliteControlStore::in_memory().unwrap());
    let agent = Arc::new(
        CodexWorkspaceAgent::from_config(CodexAppServerConfig {
            codex_binary: binary,
            ..CodexAppServerConfig::default()
        })
        .unwrap(),
    );
    let service = Arc::new(LocalControlService::with_workspace_agent(
        store.clone(),
        agent.clone(),
    ));
    let _directory = setup(&service, config("high")).await;
    let running = {
        let service = service.clone();
        tokio::spawn(async move { service.execute(send("one")).await })
    };
    let (run_id, approval_id) = pending_approval(&service).await;

    let persisted = serde_json::to_string(&store.load().await.unwrap().value).unwrap();
    for secret in [
        "header-secret",
        "auth-secret",
        "cookie-secret",
        "url-user",
        "url-secret",
    ] {
        assert!(
            !persisted.contains(secret),
            "secret reached storage: {secret}"
        );
    }

    // A desktop reconnect obtains a fresh workspace.view from the same durable daemon state.
    let reconnected_service =
        LocalControlService::with_workspace_agent(store.clone(), agent.clone());
    let reconnected = view(&reconnected_service).await;
    let approval = reconnected
        .runs
        .iter()
        .flat_map(|run| &run.native_approvals)
        .find(|approval| approval.id == approval_id)
        .unwrap();
    assert_eq!(approval.protocol_request_id, ProtocolRequestId::Integer(73));
    assert_eq!(approval.thread_id, "thread-a");
    assert_eq!(approval.turn_id, "turn-a");
    assert_eq!(approval.item_id, "command-a");
    let NativeApprovalTarget::Command { command, .. } = &approval.target else {
        panic!("expected command approval target");
    };
    assert!(command.contains("curl"));
    assert!(command.contains("example.test/v1"));
    assert!(command.contains("X-Api-Key:[REDACTED]"));
    assert!(command.contains("Authorization:[REDACTED]"));
    assert!(command.contains("Cookie:[REDACTED]"));
    for secret in [
        "header-secret",
        "auth-secret",
        "cookie-secret",
        "url-user",
        "url-secret",
    ] {
        assert!(!command.contains(secret));
    }

    let resolved = service
        .execute(Command::ResolveNativeApproval {
            run_id: run_id.clone(),
            approval_id,
            action: NativeApprovalAction::Approve,
            scope: Some(ApprovalGrantScope::OneShot),
        })
        .await;
    assert!(resolved.ok, "{:?}", resolved.error);
    let completed = tokio::time::timeout(Duration::from_secs(5), running)
        .await
        .unwrap()
        .unwrap();
    assert!(completed.ok, "{:?}", completed.error);
    let final_view = view(&service).await;
    assert_eq!(
        final_view
            .runs
            .iter()
            .find(|run| run.id == run_id)
            .unwrap()
            .status,
        "completed"
    );
    let final_rendered = format!("{final_view:?}");
    assert!(!final_rendered.contains("header-secret"));
    assert!(!final_rendered.contains("auth-secret"));
    assert!(!final_rendered.contains("cookie-secret"));
    assert!(!final_rendered.contains("url-user"));
    assert!(!final_rendered.contains("url-secret"));
}

#[tokio::test]
async fn denial_is_not_approval_and_session_grants_respect_administrator_policy() {
    let store = Arc::new(SqliteControlStore::in_memory().unwrap());
    let agent = Arc::new(ApprovalAgent::new(NativeApprovalKind::Permissions));
    let service = Arc::new(
        LocalControlService::with_workspace_agent(store, agent.clone()).with_permission_limits(
            PermissionPolicyLimits {
                max_sandbox: SandboxAccess::FullAccess,
                allow_session_approvals: false,
            },
        ),
    );
    let _directory = setup(&service, config("high")).await;
    let running = {
        let service = service.clone();
        tokio::spawn(async move { service.execute(send("one")).await })
    };
    wait_for_signal(&agent.requested).await;
    let (run_id, approval_id) = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if let Some(approval) = view(&service)
                .await
                .runs
                .iter()
                .flat_map(|run| &run.native_approvals)
                .find(|approval| approval.status == ait_domain::NativeApprovalStatus::Pending)
            {
                break (approval.run_id.clone(), approval.id.clone());
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let wrong_scope = service
        .execute(Command::ResolveNativeApproval {
            run_id: run_id.clone(),
            approval_id: approval_id.clone(),
            action: NativeApprovalAction::Approve,
            scope: Some(ApprovalGrantScope::OneShot),
        })
        .await;
    assert_eq!(
        wrong_scope.error.unwrap().code,
        ErrorCode::InvalidConfiguration
    );
    let prohibited = service
        .execute(Command::ResolveNativeApproval {
            run_id: run_id.clone(),
            approval_id: approval_id.clone(),
            action: NativeApprovalAction::Approve,
            scope: Some(ApprovalGrantScope::Session),
        })
        .await;
    assert_eq!(
        prohibited.error.unwrap().code,
        ErrorCode::InvalidConfiguration
    );
    let denied = service
        .execute(Command::ResolveNativeApproval {
            run_id,
            approval_id,
            action: NativeApprovalAction::Deny,
            scope: None,
        })
        .await;
    assert!(denied.ok, "{:?}", denied.error);
    assert!(running.await.unwrap().ok);
    assert_eq!(
        *agent.decision.lock().unwrap(),
        Some(WorkspaceApprovalDecision::Denied)
    );
}

#[tokio::test]
async fn permission_write_grants_cannot_exceed_run_or_administrator_ceiling() {
    reject_permission_write_for_read_only_run(SandboxAccess::ReadOnly).await;
    reject_permission_write_for_read_only_run(SandboxAccess::FullAccess).await;
}

async fn reject_permission_write_for_read_only_run(max_sandbox: SandboxAccess) {
    let store = Arc::new(SqliteControlStore::in_memory().unwrap());
    let agent = Arc::new(ApprovalAgent::new(NativeApprovalKind::Permissions));
    let service = Arc::new(
        LocalControlService::with_workspace_agent(store, agent.clone()).with_permission_limits(
            PermissionPolicyLimits {
                max_sandbox,
                allow_session_approvals: true,
            },
        ),
    );
    let _directory = setup(&service, config("high")).await;
    let running = {
        let service = service.clone();
        tokio::spawn(async move { service.execute(send("one")).await })
    };
    wait_for_signal(&agent.requested).await;
    let (run_id, approval_id) = pending_approval(&service).await;
    let rejected = service
        .execute(Command::ResolveNativeApproval {
            run_id: run_id.clone(),
            approval_id: approval_id.clone(),
            action: NativeApprovalAction::Approve,
            scope: Some(ApprovalGrantScope::Turn),
        })
        .await;
    assert_eq!(
        rejected.error.unwrap().code,
        ErrorCode::InvalidConfiguration
    );
    let pending = view(&service)
        .await
        .runs
        .into_iter()
        .find(|run| run.id == run_id)
        .unwrap();
    assert_eq!(
        pending.native_approvals[0].status,
        ait_domain::NativeApprovalStatus::Pending
    );
    assert!(pending.native_approvals[0].granted_permissions.is_none());
    assert!(agent.decision.lock().unwrap().is_none());
    let cancelled = service
        .execute(Command::ResolveNativeApproval {
            run_id,
            approval_id,
            action: NativeApprovalAction::Cancel,
            scope: None,
        })
        .await;
    assert!(cancelled.ok, "{:?}", cancelled.error);
    tokio::time::timeout(Duration::from_secs(3), running)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn workspace_write_permission_grants_cannot_escape_the_project() {
    let outside = tempfile::tempdir().unwrap();
    let store = Arc::new(SqliteControlStore::in_memory().unwrap());
    let agent = Arc::new(ApprovalAgent::permissions_at(
        outside.path().join("escaped.txt").to_string_lossy(),
    ));
    let service = Arc::new(LocalControlService::with_workspace_agent(
        store,
        agent.clone(),
    ));
    let _directory = setup(&service, config("high")).await;
    save_permission_settings(&service, "workspace_write", "on_request").await;
    let running = {
        let service = service.clone();
        tokio::spawn(async move { service.execute(send("one")).await })
    };
    wait_for_signal(&agent.requested).await;
    let (run_id, approval_id) = pending_approval(&service).await;
    let rejected = service
        .execute(Command::ResolveNativeApproval {
            run_id: run_id.clone(),
            approval_id: approval_id.clone(),
            action: NativeApprovalAction::Approve,
            scope: Some(ApprovalGrantScope::Turn),
        })
        .await;
    assert_eq!(
        rejected.error.unwrap().code,
        ErrorCode::InvalidConfiguration
    );
    assert!(!outside.path().join("escaped.txt").exists());
    let pending = view(&service)
        .await
        .runs
        .into_iter()
        .find(|run| run.id == run_id)
        .unwrap();
    assert_eq!(
        pending.native_approvals[0].status,
        ait_domain::NativeApprovalStatus::Pending
    );
    assert!(pending.native_approvals[0].granted_permissions.is_none());
    let cancelled = service
        .execute(Command::ResolveNativeApproval {
            run_id,
            approval_id,
            action: NativeApprovalAction::Cancel,
            scope: None,
        })
        .await;
    assert!(cancelled.ok, "{:?}", cancelled.error);
    tokio::time::timeout(Duration::from_secs(3), running)
        .await
        .unwrap()
        .unwrap();
}

async fn pending_approval(service: &LocalControlService) -> (String, String) {
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if let Some(approval) = view(service)
                .await
                .runs
                .iter()
                .flat_map(|run| &run.native_approvals)
                .find(|approval| approval.status == ait_domain::NativeApprovalStatus::Pending)
            {
                break (approval.run_id.clone(), approval.id.clone());
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn cancelling_each_native_approval_kind_cancels_the_run_without_hanging() {
    for kind in [
        NativeApprovalKind::CommandExecution,
        NativeApprovalKind::FileChange,
        NativeApprovalKind::Permissions,
    ] {
        let store = Arc::new(SqliteControlStore::in_memory().unwrap());
        let agent = Arc::new(ApprovalAgent::new(kind));
        let service = Arc::new(LocalControlService::with_workspace_agent(
            store,
            agent.clone(),
        ));
        let _directory = setup(&service, config("high")).await;
        let running = {
            let service = service.clone();
            tokio::spawn(async move { service.execute(send("one")).await })
        };
        wait_for_signal(&agent.requested).await;
        let (run_id, approval_id) = tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                if let Some(run) = view(&service)
                    .await
                    .runs
                    .into_iter()
                    .find(|run| !run.native_approvals.is_empty())
                {
                    break (run.id, run.native_approvals[0].id.clone());
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let cancelled = service
            .execute(Command::ResolveNativeApproval {
                run_id: run_id.clone(),
                approval_id: approval_id.clone(),
                action: NativeApprovalAction::Cancel,
                scope: None,
            })
            .await;
        assert!(cancelled.ok, "{kind:?}: {:?}", cancelled.error);
        tokio::time::timeout(Duration::from_secs(3), running)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            *agent.decision.lock().unwrap(),
            Some(WorkspaceApprovalDecision::Cancelled),
            "{kind:?}"
        );
        let snapshot = view(&service).await;
        let run = snapshot
            .runs
            .into_iter()
            .find(|run| run.id == run_id)
            .unwrap();
        assert_eq!(run.status, "cancelled", "{kind:?}");
        assert_eq!(
            run.native_approvals[0].status,
            ait_domain::NativeApprovalStatus::Cancelled,
            "{kind:?}"
        );
        assert!(
            snapshot
                .messages
                .iter()
                .all(|message| message.role != "assistant"),
            "{kind:?}"
        );
        let duplicate = service
            .execute(Command::ResolveNativeApproval {
                run_id,
                approval_id,
                action: NativeApprovalAction::Approve,
                scope: Some(if kind == NativeApprovalKind::Permissions {
                    ApprovalGrantScope::Turn
                } else {
                    ApprovalGrantScope::OneShot
                }),
            })
            .await;
        assert_eq!(
            duplicate.error.unwrap().code,
            ErrorCode::RunAlreadyTerminal,
            "{kind:?}"
        );
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
#[async_trait]
impl AgentProviderGateway for Gateway {
    async fn store_secret(&self, reference: &str, secret: &str) -> Result<(), DomainError> {
        self.secrets
            .lock()
            .unwrap()
            .insert(reference.into(), secret.into());
        Ok(())
    }
    async fn delete_secret(&self, reference: &str) -> Result<(), DomainError> {
        self.secrets.lock().unwrap().remove(reference);
        Ok(())
    }
    async fn list_models(
        &self,
        _: &AgentProvider,
        reference: &str,
    ) -> Result<Vec<ProviderModel>, DomainError> {
        assert!(self.secrets.lock().unwrap().contains_key(reference));
        if self.omit_models.load(std::sync::atomic::Ordering::Relaxed) {
            return Ok(Vec::new());
        }
        Ok(vec![
            ProviderModel {
                id: "chat".into(),
                name: "Chat".into(),
                reasoning_efforts: Vec::new(),
            },
            ProviderModel {
                id: "new".into(),
                name: "New".into(),
                reasoning_efforts: Vec::new(),
            },
        ])
    }
    async fn list_models_with_secret(
        &self,
        _: &AgentProvider,
        secret: &str,
    ) -> Result<Vec<ProviderModel>, DomainError> {
        if secret == "invalid-preview-secret" {
            return Err(DomainError::invariant(
                ErrorCode::ProviderFailed,
                "model discovery failed",
            ));
        }
        assert!(!secret.is_empty());
        Ok(vec![
            ProviderModel {
                id: "chat".into(),
                name: "Chat".into(),
                reasoning_efforts: Vec::new(),
            },
            ProviderModel {
                id: "new".into(),
                name: "New".into(),
                reasoning_efforts: Vec::new(),
            },
        ])
    }
    async fn complete(
        &self,
        _: &AgentProvider,
        reference: &str,
        config: &AgentConfiguration,
        messages: Vec<ProviderMessage>,
    ) -> Result<String, DomainError> {
        assert!(self.secrets.lock().unwrap().contains_key(reference));
        self.calls.lock().unwrap().push((
            config.clone(),
            messages.into_iter().map(|m| m.role).collect(),
        ));
        Ok("API response".into())
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

#[tokio::test]
async fn fresh_workspace_exposes_only_the_codex_builtin() {
    let service = LocalControlService::new(Arc::new(SqliteControlStore::in_memory().unwrap()));
    let providers = view(&service).await.providers;
    assert_eq!(providers.len(), 1);
    assert_eq!(providers[0].provider.id, "builtin-codex");
    assert_eq!(providers[0].provider.kind, AgentMode::Codex);
    for kind in RETIRED_BUILTINS {
        assert!(serde_json::from_value::<AgentMode>(serde_json::json!(kind)).is_err());
    }
}

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

const RETIRED_BUILTINS: [&str; 4] = ["tool", "manual", "provider_failure", "approval_required"];

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

struct PausingStore {
    inner: SqliteControlStore,
    entered: Semaphore,
    release: Semaphore,
}

#[async_trait]
impl ControlStore for PausingStore {
    async fn read(
        &self,
        filters: &[ait_ports::ControlFilter],
    ) -> Result<ait_ports::ControlRead, ait_ports::ControlStoreError> {
        self.inner.read(filters).await
    }
    async fn replay(
        &self,
        after: u64,
        limit: usize,
    ) -> Result<Vec<ait_ports::DurableEvent>, ait_ports::ControlStoreError> {
        self.inner.replay(after, limit).await
    }
    async fn event_bounds(&self) -> Result<ait_ports::EventBounds, ait_ports::ControlStoreError> {
        self.inner.event_bounds().await
    }
    async fn replay_page(
        &self,
        after: u64,
        limit: usize,
    ) -> Result<ait_ports::DurableEventPage, ait_ports::ControlStoreError> {
        self.inner.replay_page(after, limit).await
    }
    async fn save_progress(
        &self,
        checkpoint: ait_ports::ProgressCheckpoint,
        events: Vec<ait_ports::PendingEvent>,
    ) -> Result<(), ait_ports::ControlStoreError> {
        self.inner.save_progress(checkpoint, events).await
    }
    async fn load_progress(
        &self,
    ) -> Result<Vec<ait_ports::ProgressCheckpoint>, ait_ports::ControlStoreError> {
        self.inner.load_progress().await
    }
    async fn clear_progress(&self, run_id: &str) -> Result<(), ait_ports::ControlStoreError> {
        self.inner.clear_progress(run_id).await
    }
    async fn apply(
        &self,
        revision: u64,
        changes: Vec<ait_ports::ControlChange>,
        events: Vec<ait_ports::PendingEvent>,
    ) -> Result<u64, ait_ports::ControlStoreError> {
        if terminal_run_status(&changes) == Some("queued") {
            self.entered.add_permits(1);
            self.release.acquire().await.unwrap().forget();
        }
        self.inner.apply(revision, changes, events).await
    }
}

#[tokio::test]
async fn pessimistic_admission_rejects_a_competing_send_before_the_first_run_is_committed() {
    let store = Arc::new(PausingStore {
        inner: SqliteControlStore::in_memory().unwrap(),
        entered: Semaphore::new(0),
        release: Semaphore::new(0),
    });
    let service = Arc::new(LocalControlService::new(store.clone()));
    let _directory = setup(&service, config("high")).await;
    let first = {
        let service = service.clone();
        tokio::spawn(async move { ok(&service, send("one")).await })
    };
    tokio::time::timeout(Duration::from_secs(3), store.entered.acquire())
        .await
        .unwrap()
        .unwrap()
        .forget();
    assert!(view(&service).await.runs.is_empty());
    let rejected = tokio::time::timeout(Duration::from_millis(500), service.execute(send("one")))
        .await
        .unwrap();
    assert_eq!(rejected.error.unwrap().code, ErrorCode::SessionBusy);
    assert_eq!(view(&service).await.messages.len(), 1);
    store.release.add_permits(1);
    first.await.unwrap();
    assert_eq!(view(&service).await.runs.len(), 1);
}
