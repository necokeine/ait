//! Cross-layer recovery tests using the production Codex Git adapter.
#![allow(clippy::pedantic)]

mod support;

use std::{
    fs,
    path::{Path, PathBuf},
    process::Command as ProcessCommand,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use ait_agent_adapters::{
    AdapterError, AgentAdapter, AgentCapabilities, AgentEvent, AgentRunRequest, AgentRunStatus,
    AgentStream, codex::CodexWorkspaceAgent,
};
use ait_application::LocalControlService;
use ait_contracts::{Command, CommandResult, RunView, default_settings};
use ait_domain::{DomainError, ErrorCode};
use ait_ports::{
    ControlStore, WorkspaceAgent, WorkspaceAgentInvocation, WorkspaceAgentResponse,
    WorkspaceIntegrationCheckpoint, WorkspaceIntegrationGate, WorkspaceProgressReporter,
    WorkspaceResultSink,
};
use ait_storage_sqlite::SqliteControlStore;
use async_trait::async_trait;
use futures_util::stream;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tempfile::TempDir;
use tokio::sync::Semaphore;

use support::{ControlStoreTestExt, WorkspaceView, workspace as read_workspace};

#[derive(Debug, Default)]
struct ScriptedAdapter {
    calls: AtomicUsize,
}

#[async_trait]
impl AgentAdapter for ScriptedAdapter {
    fn driver(&self) -> &'static str {
        "recovery-fixture"
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
        let call = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
        let file = format!("agent-change-{call}.txt");
        fs::write(request.cwd.join(&file), format!("change {call}\n")).unwrap();
        let text = format!("created {file}");
        Ok(Box::pin(stream::iter([
            Ok(AgentEvent::ItemCompleted {
                item: json!({
                    "type": "agentMessage",
                    "id": format!("answer-{call}"),
                    "phase": "final_answer",
                    "text": text,
                }),
            }),
            Ok(AgentEvent::Completed {
                turn_id: format!("turn-{call}"),
                status: AgentRunStatus::Completed,
                error: None,
            }),
        ])))
    }
}

#[derive(Debug)]
struct CompositeGate {
    durable: Arc<dyn WorkspaceIntegrationGate>,
    injected: Arc<dyn WorkspaceIntegrationGate>,
}

#[async_trait]
impl WorkspaceIntegrationGate for CompositeGate {
    async fn begin_integration(&self) -> Result<(), DomainError> {
        self.durable.begin_integration().await?;
        self.injected.begin_integration().await
    }

    async fn checkpoint(
        &self,
        checkpoint: WorkspaceIntegrationCheckpoint,
    ) -> Result<(), DomainError> {
        self.durable.checkpoint(checkpoint).await?;
        self.injected.checkpoint(checkpoint).await
    }
}

#[derive(Clone)]
struct GateWrappedAgent {
    inner: CodexWorkspaceAgent,
    injected: Arc<dyn WorkspaceIntegrationGate>,
}

impl std::fmt::Debug for GateWrappedAgent {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("GateWrappedAgent").finish()
    }
}

impl GateWrappedAgent {
    fn wrap(&self, request: &mut WorkspaceAgentInvocation) {
        if let Some(durable) = request.integration_gate.take() {
            request.integration_gate = Some(Arc::new(CompositeGate {
                durable,
                injected: self.injected.clone(),
            }));
        } else {
            request.integration_gate = Some(self.injected.clone());
        }
    }
}

#[async_trait]
impl WorkspaceAgent for GateWrappedAgent {
    async fn invoke(
        &self,
        mut request: WorkspaceAgentInvocation,
    ) -> Result<WorkspaceAgentResponse, DomainError> {
        self.wrap(&mut request);
        self.inner.invoke(request).await
    }

    async fn invoke_with_progress_and_checkpoint(
        &self,
        mut request: WorkspaceAgentInvocation,
        progress: Arc<dyn WorkspaceProgressReporter>,
        result_sink: &dyn WorkspaceResultSink,
    ) -> Result<WorkspaceAgentResponse, DomainError> {
        self.wrap(&mut request);
        self.inner
            .invoke_with_progress_and_checkpoint(request, progress, result_sink)
            .await
    }

    async fn recover_checkpointed(
        &self,
        mut request: WorkspaceAgentInvocation,
        result: WorkspaceAgentResponse,
        baseline_ref: Option<String>,
    ) -> Result<WorkspaceAgentResponse, DomainError> {
        self.wrap(&mut request);
        self.inner
            .recover_checkpointed(request, result, baseline_ref)
            .await
    }
}

struct CheckpointPausingAgent {
    inner: CodexWorkspaceAgent,
    entered: Semaphore,
    release: Semaphore,
}

impl std::fmt::Debug for CheckpointPausingAgent {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("CheckpointPausingAgent").finish()
    }
}

impl CheckpointPausingAgent {
    async fn wait_until_checkpoint(&self) {
        tokio::time::timeout(Duration::from_secs(3), self.entered.acquire())
            .await
            .unwrap()
            .unwrap()
            .forget();
    }
}

struct PausingResultSink<'a> {
    inner: &'a dyn WorkspaceResultSink,
    entered: &'a Semaphore,
    release: &'a Semaphore,
}

#[async_trait]
impl WorkspaceResultSink for PausingResultSink<'_> {
    async fn checkpoint(&self, result: WorkspaceAgentResponse) -> Result<(), DomainError> {
        self.entered.add_permits(1);
        self.release.acquire().await.unwrap().forget();
        self.inner.checkpoint(result).await
    }
}

#[async_trait]
impl WorkspaceAgent for CheckpointPausingAgent {
    async fn invoke(
        &self,
        request: WorkspaceAgentInvocation,
    ) -> Result<WorkspaceAgentResponse, DomainError> {
        self.inner.invoke(request).await
    }

    async fn invoke_with_progress_and_checkpoint(
        &self,
        request: WorkspaceAgentInvocation,
        progress: Arc<dyn WorkspaceProgressReporter>,
        result_sink: &dyn WorkspaceResultSink,
    ) -> Result<WorkspaceAgentResponse, DomainError> {
        let paused = PausingResultSink {
            inner: result_sink,
            entered: &self.entered,
            release: &self.release,
        };
        self.inner
            .invoke_with_progress_and_checkpoint(request, progress, &paused)
            .await
    }
}

#[derive(Debug)]
struct FaultGate {
    failures: Vec<WorkspaceIntegrationCheckpoint>,
}

#[async_trait]
impl WorkspaceIntegrationGate for FaultGate {
    async fn begin_integration(&self) -> Result<(), DomainError> {
        Ok(())
    }

    async fn checkpoint(
        &self,
        checkpoint: WorkspaceIntegrationCheckpoint,
    ) -> Result<(), DomainError> {
        if self.failures.contains(&checkpoint) {
            return Err(DomainError::invariant(
                ErrorCode::RunRecoveryFailed,
                format!("injected integration failure at {checkpoint:?}"),
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PauseAt {
    Begin,
    Checkpoint(WorkspaceIntegrationCheckpoint),
}

#[derive(Debug)]
struct PausingGate {
    at: PauseAt,
    entered: Semaphore,
    release: Semaphore,
}

impl PausingGate {
    fn new(at: PauseAt) -> Self {
        Self {
            at,
            entered: Semaphore::new(0),
            release: Semaphore::new(0),
        }
    }

    async fn wait_until_entered(&self) {
        tokio::time::timeout(Duration::from_secs(3), self.entered.acquire())
            .await
            .unwrap()
            .unwrap()
            .forget();
    }

    async fn pause(&self) {
        self.entered.add_permits(1);
        self.release.acquire().await.unwrap().forget();
    }
}

#[async_trait]
impl WorkspaceIntegrationGate for PausingGate {
    async fn begin_integration(&self) -> Result<(), DomainError> {
        if self.at == PauseAt::Begin {
            self.pause().await;
        }
        Ok(())
    }

    async fn checkpoint(
        &self,
        checkpoint: WorkspaceIntegrationCheckpoint,
    ) -> Result<(), DomainError> {
        if self.at == PauseAt::Checkpoint(checkpoint) {
            self.pause().await;
        }
        Ok(())
    }
}

fn production_agent(adapter: Arc<ScriptedAdapter>) -> CodexWorkspaceAgent {
    CodexWorkspaceAgent::new(adapter)
}

fn wrapped_agent(
    adapter: Arc<ScriptedAdapter>,
    gate: Arc<dyn WorkspaceIntegrationGate>,
) -> Arc<dyn WorkspaceAgent> {
    Arc::new(GateWrappedAgent {
        inner: production_agent(adapter),
        injected: gate,
    })
}

async fn ok(service: &LocalControlService, command: Command) -> CommandResult {
    let response = service.execute(command).await;
    assert!(response.ok, "{:?}", response.error);
    response.result.unwrap()
}

async fn workspace(service: &LocalControlService) -> WorkspaceView {
    read_workspace(service).await
}

async fn register_agent(service: &LocalControlService) {
    let mut settings = default_settings();
    settings
        .0
        .insert("permissions.sandbox".into(), json!("workspace_write"));
    ok(
        service,
        Command::SaveSettings {
            expected_revision: 1,
            values: settings,
        },
    )
    .await;
    ok(
        service,
        Command::RegisterAgent {
            id: "codex".into(),
            name: "Codex".into(),
            config: ait_contracts::AgentConfiguration {
                provider_id: "builtin-codex".into(),
                model: "gpt-5.6-sol".into(),
                reasoning_effort: Some("high".into()),
            },
        },
    )
    .await;
}

async fn register_project(service: &LocalControlService, root: &Path, id: &str) -> PathBuf {
    let path = root.join(id);
    fs::create_dir(&path).unwrap();
    ok(
        service,
        Command::RegisterProject {
            id: id.into(),
            name: id.into(),
            workdir: Some(path.display().to_string()),
            repo_url: None,
        },
    )
    .await;
    ok(
        service,
        Command::CreateSession {
            id: format!("session-{id}"),
            project_id: id.into(),
            agent_id: "codex".into(),
            at_message_id: None,
        },
    )
    .await;
    path
}

async fn send(service: &LocalControlService, project_id: &str) -> RunView {
    let CommandResult::Run(run) = ok(
        service,
        Command::SendMessage {
            session_id: format!("session-{project_id}"),
            text: format!("change {project_id}"),
        },
    )
    .await
    else {
        panic!("expected Run")
    };
    run
}

async fn session_worktree(service: &LocalControlService, project_id: &str) -> PathBuf {
    PathBuf::from(
        workspace(service)
            .await
            .sessions
            .into_iter()
            .find(|session| session.id == format!("session-{project_id}"))
            .unwrap()
            .workdir,
    )
}

fn git(path: &Path, arguments: &[&str]) -> String {
    let output = ProcessCommand::new("git")
        .arg("-C")
        .arg(path)
        .args(arguments)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {arguments:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

fn git_head(path: &Path) -> String {
    git(path, &["rev-parse", "HEAD"])
}

fn run_ref(run_id: &str) -> String {
    format!("refs/ait/runs/{:x}", Sha256::digest(run_id.as_bytes()))
}

fn journal_commit(value: &Value, run_id: &str) -> String {
    value["workspace_run_journals"][run_id]["result"]["commit_id"]
        .as_str()
        .unwrap()
        .to_owned()
}

async fn rewind_completed_runs(store: &SqliteControlStore, runs: &[RunView]) {
    let snapshot = store.load().await.unwrap();
    let mut value = snapshot.value;
    for completed in runs {
        let run = value["runs"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|run| run["id"] == completed.id)
            .unwrap();
        run["status"] = Value::String("settling".into());
        run["phase"] = Value::String("result_persisted".into());
        run["last_message_id"] = Value::Null;
        run["error"] = Value::Null;
        let session = value["sessions"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|session| session["id"] == completed.session_id.as_deref().unwrap())
            .unwrap();
        session["active_run_id"] = Value::String(completed.id.clone());
        session["current_message_id"] = Value::String(completed.base_message_id.clone());
    }
    store
        .commit(snapshot.revision, value, Vec::new())
        .await
        .unwrap();
}

fn assistant_count(workspace: &WorkspaceView, run: &RunView) -> usize {
    workspace
        .messages
        .iter()
        .filter(|message| {
            message.role == "assistant"
                && message.parent_message_id.as_deref() == Some(run.base_message_id.as_str())
        })
        .count()
}

#[tokio::test]
async fn post_gate_git_failures_never_turn_an_unpublished_result_into_completed() {
    let cases = [
        (vec![WorkspaceIntegrationCheckpoint::BeforeRefPublish], true),
        (
            vec![WorkspaceIntegrationCheckpoint::BeforeIndexPublish],
            true,
        ),
        (
            vec![
                WorkspaceIntegrationCheckpoint::BeforeIndexPublish,
                WorkspaceIntegrationCheckpoint::AfterRollbackCandidateQuarantine,
            ],
            true,
        ),
    ];
    for (failures, expect_rollback_material) in cases {
        let temporary = TempDir::new().unwrap();
        let store = Arc::new(SqliteControlStore::in_memory().unwrap());
        let adapter = Arc::new(ScriptedAdapter::default());
        let agent = wrapped_agent(adapter, Arc::new(FaultGate { failures }));
        let service = LocalControlService::with_workspace_agent(store.clone(), agent);
        register_agent(&service).await;
        register_project(&service, temporary.path(), "project").await;
        let worktree = session_worktree(&service, "project").await;
        let baseline = git_head(&worktree);

        let run = send(&service, "project").await;
        assert_eq!(run.status, "interrupted");
        assert_ne!(run.status, "completed");
        assert!(
            run.error
                .as_ref()
                .unwrap()
                .message
                .contains("injected integration failure")
        );
        assert_eq!(git_head(&worktree), baseline);
        let view = workspace(&service).await;
        assert_eq!(assistant_count(&view, &run), 0);
        assert!(view.sessions[0].active_run_id.is_none());

        let git_dir = PathBuf::from(git(&worktree, &["rev-parse", "--absolute-git-dir"]));
        let rollback_root = git_dir.join("ait").join("integration-rollbacks");
        assert_eq!(rollback_root.exists(), expect_rollback_material);
    }
}

#[derive(Clone, Copy, Debug)]
enum RecoveryMismatch {
    Branch,
    RunRef,
    Index,
    Rollback,
}

fn inject_recovery_mismatch(
    mismatch: RecoveryMismatch,
    project: &Path,
    run: &RunView,
    commit: &str,
) {
    let baseline = run.workspace_base_commit.as_deref().unwrap();
    match mismatch {
        RecoveryMismatch::Branch => {
            git(project, &["checkout", "-b", "recovery-mismatch"]);
        }
        RecoveryMismatch::RunRef => {
            git(project, &["reset", "--hard", baseline]);
            git(project, &["update-ref", &run_ref(&run.id), baseline]);
        }
        RecoveryMismatch::Index => {
            fs::write(project.join("external.txt"), "external\n").unwrap();
            git(project, &["add", "external.txt"]);
        }
        RecoveryMismatch::Rollback => {
            git(project, &["reset", "--hard", baseline]);
            git(project, &["update-ref", &run_ref(&run.id), commit]);
            let git_dir = PathBuf::from(git(project, &["rev-parse", "--absolute-git-dir"]));
            fs::create_dir_all(
                git_dir
                    .join("ait")
                    .join("integration-rollbacks")
                    .join(commit),
            )
            .unwrap();
        }
    }
}

#[tokio::test]
async fn ambiguous_git_recovery_interrupts_only_its_run_and_healthy_recovery_continues() {
    for mismatch in [
        RecoveryMismatch::Branch,
        RecoveryMismatch::RunRef,
        RecoveryMismatch::Index,
        RecoveryMismatch::Rollback,
    ] {
        let temporary = TempDir::new().unwrap();
        let store = Arc::new(SqliteControlStore::in_memory().unwrap());
        let adapter = Arc::new(ScriptedAdapter::default());
        let initial = LocalControlService::with_workspace_agent(
            store.clone(),
            Arc::new(production_agent(adapter.clone())),
        );
        register_agent(&initial).await;
        register_project(&initial, temporary.path(), "bad").await;
        register_project(&initial, temporary.path(), "healthy").await;
        let bad_worktree = session_worktree(&initial, "bad").await;
        let bad = send(&initial, "bad").await;
        let healthy = send(&initial, "healthy").await;
        let state = store.load().await.unwrap().value;
        let bad_commit = journal_commit(&state, &bad.id);
        rewind_completed_runs(&store, &[bad.clone(), healthy.clone()]).await;
        inject_recovery_mismatch(mismatch, &bad_worktree, &bad, &bad_commit);
        let calls = adapter.calls.load(Ordering::SeqCst);
        let recovery = LocalControlService::with_workspace_agent(
            store.clone(),
            Arc::new(production_agent(adapter.clone())),
        );

        let recovered = recovery.recover_interrupted_runs().await.unwrap();
        assert_eq!(recovered.len(), 2, "mismatch: {mismatch:?}");
        assert_eq!(adapter.calls.load(Ordering::SeqCst), calls);
        let view = workspace(&recovery).await;
        let bad_after = view.runs.iter().find(|run| run.id == bad.id).unwrap();
        let healthy_after = view.runs.iter().find(|run| run.id == healthy.id).unwrap();
        assert_eq!(bad_after.status, "interrupted", "mismatch: {mismatch:?}");
        assert_eq!(healthy_after.status, "completed", "mismatch: {mismatch:?}");
        // Rewinding the Run cannot erase its immutable historical Message.
        assert_eq!(assistant_count(&view, &bad), 1);
        assert_eq!(assistant_count(&view, &healthy), 2);
        assert!(
            view.sessions
                .iter()
                .all(|session| session.active_run_id.is_none())
        );
    }
}

#[tokio::test]
async fn already_published_recovery_claims_finalization_before_cancel_can_win() {
    let temporary = TempDir::new().unwrap();
    let store = Arc::new(SqliteControlStore::in_memory().unwrap());
    let adapter = Arc::new(ScriptedAdapter::default());
    let initial = LocalControlService::with_workspace_agent(
        store.clone(),
        Arc::new(production_agent(adapter.clone())),
    );
    register_agent(&initial).await;
    register_project(&initial, temporary.path(), "project").await;
    let completed = send(&initial, "project").await;
    rewind_completed_runs(&store, std::slice::from_ref(&completed)).await;
    let before_cursor = store.event_bounds().await.unwrap().latest.unwrap_or(0);
    let pause = Arc::new(PausingGate::new(PauseAt::Begin));
    let recovery = Arc::new(LocalControlService::with_workspace_agent(
        store.clone(),
        wrapped_agent(adapter.clone(), pause.clone()),
    ));
    let task = {
        let recovery = recovery.clone();
        tokio::spawn(async move { recovery.recover_interrupted_runs().await })
    };
    pause.wait_until_entered().await;
    let integrating = workspace(&recovery).await;
    let run = integrating
        .runs
        .iter()
        .find(|run| run.id == completed.id)
        .unwrap();
    assert_eq!(run.phase.as_deref(), Some("integrating"));

    let independent = LocalControlService::new(store.clone());
    let cancelled = independent
        .execute(Command::CancelRun {
            run_id: completed.id.clone(),
        })
        .await;
    assert_eq!(cancelled.error.unwrap().code, ErrorCode::RunAlreadyTerminal);
    pause.release.add_permits(1);
    let recovered = task.await.unwrap().unwrap();
    assert_eq!(recovered[0].status, "completed");
    let view = workspace(&recovery).await;
    let final_run = view.runs.iter().find(|run| run.id == completed.id).unwrap();
    assert_eq!(final_run.status, "completed");
    assert_eq!(assistant_count(&view, &completed), 2);
    let assistant = view
        .messages
        .iter()
        .find(|message| Some(&message.id) == final_run.last_message_id.as_ref())
        .unwrap();
    assert_eq!(
        assistant.data.as_ref().unwrap()["codex"]["commit_id"],
        store.load().await.unwrap().value["workspace_run_journals"][&completed.id]["result"]["commit_id"]
    );
    let events = store.replay(before_cursor, 100).await.unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|event| {
                event.kind == "run.updated"
                    && event.entity_id.as_deref() == Some(completed.id.as_str())
                    && event.body["status"] == "completed"
            })
            .count(),
        1
    );
}

#[tokio::test]
async fn startup_recovery_cannot_steal_a_live_publishers_lease() {
    let temporary = TempDir::new().unwrap();
    let store = Arc::new(SqliteControlStore::in_memory().unwrap());
    let adapter = Arc::new(ScriptedAdapter::default());
    let pause = Arc::new(PausingGate::new(PauseAt::Checkpoint(
        WorkspaceIntegrationCheckpoint::BeforeIndexLock,
    )));
    let owner = Arc::new(LocalControlService::with_workspace_agent(
        store.clone(),
        wrapped_agent(adapter.clone(), pause.clone()),
    ));
    register_agent(&owner).await;
    register_project(&owner, temporary.path(), "project").await;
    let worktree = session_worktree(&owner, "project").await;
    let baseline = git_head(&worktree);
    let execution = {
        let owner = owner.clone();
        tokio::spawn(async move { send(&owner, "project").await })
    };
    pause.wait_until_entered().await;
    let before = store.load().await.unwrap();
    let active = before.value["runs"][0]["id"].as_str().unwrap().to_owned();
    assert_eq!(before.value["runs"][0]["phase"], "integrating");

    let contender = LocalControlService::with_workspace_agent(
        store.clone(),
        Arc::new(production_agent(adapter.clone())),
    );
    assert!(
        contender
            .recover_interrupted_runs()
            .await
            .unwrap()
            .is_empty()
    );
    let after_contender = store.load().await.unwrap();
    assert_eq!(after_contender.revision, before.revision);
    assert_eq!(
        after_contender.value["runs"][0]["lease_epoch"],
        before.value["runs"][0]["lease_epoch"]
    );
    assert_eq!(after_contender.value["runs"][0]["phase"], "integrating");

    pause.release.add_permits(1);
    let finished = execution.await.unwrap();
    assert_eq!(finished.id, active);
    assert_eq!(finished.status, "completed");
    assert_ne!(git_head(&worktree), baseline);
    assert_eq!(adapter.calls.load(Ordering::SeqCst), 1);
    let view = workspace(&owner).await;
    assert_eq!(assistant_count(&view, &finished), 1);
    assert!(view.sessions[0].active_run_id.is_none());
}

#[tokio::test]
async fn stale_result_sink_is_fenced_after_cancel_without_git_side_effects() {
    let temporary = TempDir::new().unwrap();
    let store = Arc::new(SqliteControlStore::in_memory().unwrap());
    let adapter = Arc::new(ScriptedAdapter::default());
    let agent = Arc::new(CheckpointPausingAgent {
        inner: production_agent(adapter.clone()),
        entered: Semaphore::new(0),
        release: Semaphore::new(0),
    });
    let service = Arc::new(LocalControlService::with_workspace_agent(
        store.clone(),
        agent.clone(),
    ));
    register_agent(&service).await;
    register_project(&service, temporary.path(), "project").await;
    let worktree = session_worktree(&service, "project").await;
    let baseline = git_head(&worktree);
    let execution = {
        let service = service.clone();
        tokio::spawn(async move { send(&service, "project").await })
    };
    agent.wait_until_checkpoint().await;
    let active = workspace(&service).await.runs[0].clone();
    assert_eq!(active.status, "running");

    let independent = LocalControlService::new(store.clone());
    let cancelled = independent
        .execute(Command::CancelRun {
            run_id: active.id.clone(),
        })
        .await;
    assert!(cancelled.ok);
    agent.release.add_permits(1);
    let terminal = execution.await.unwrap();
    assert_eq!(terminal.status, "cancelled");
    assert_eq!(git_head(&worktree), baseline);
    let view = workspace(&service).await;
    assert_eq!(assistant_count(&view, &active), 0);
    assert!(view.sessions[0].active_run_id.is_none());
    assert_eq!(adapter.calls.load(Ordering::SeqCst), 1);
}
