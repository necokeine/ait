//! Command execution, durable checkpoints, and read-only Run queries.

use std::{
    collections::{BTreeMap, VecDeque},
    process::Command as ProcessCommand,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    },
};

use ait_application::LocalControlService;
use ait_contracts::{Command, CommandResult, ProjectView, RunView};
use ait_domain::{DomainError, ErrorCode};
use ait_ports::{
    ControlSnapshot, ControlStore, ControlStoreError, DurableEvent, DurableEventPage, EventBounds,
    PendingEvent, ProgressCheckpoint, RunOutputArchive, WorkspaceAgent, WorkspaceAgentInvocation,
    WorkspaceAgentResponse, WorkspaceOperation, WorkspaceProgressEvent, WorkspaceProgressReporter,
};
use ait_storage_sqlite::SqliteControlStore;
use async_trait::async_trait;
use serde_json::Value;
use sha2::{Digest as _, Sha256};
use tempfile::TempDir;
use tokio::{sync::Semaphore, time::Duration};

struct ConflictingStore {
    inner: SqliteControlStore,
    checkpoints: Mutex<VecDeque<&'static str>>,
    reject_runs: AtomicBool,
    progress_delay_ms: AtomicU64,
    fail_progress: AtomicBool,
}

impl ConflictingStore {
    fn rejects(&self, value: &Value) -> bool {
        let status = value["runs"]
            .as_array()
            .and_then(|runs| runs.last())
            .and_then(|run| run["status"].as_str());
        let mut checkpoints = self.checkpoints.lock().unwrap();
        if status.is_some() && status == checkpoints.front().copied() {
            checkpoints.pop_front();
            true
        } else {
            status.is_some() && self.reject_runs.load(Ordering::Relaxed)
        }
    }
}

#[async_trait]
impl ControlStore for ConflictingStore {
    async fn load(&self) -> Result<ControlSnapshot, ControlStoreError> {
        self.inner.load().await
    }

    async fn commit(
        &self,
        revision: u64,
        value: Value,
        events: Vec<PendingEvent>,
    ) -> Result<ControlSnapshot, ControlStoreError> {
        if self.rejects(&value) {
            return Err(ControlStoreError::Conflict);
        }
        self.inner.commit(revision, value, events).await
    }

    async fn commit_terminal(
        &self,
        revision: u64,
        value: Value,
        events: Vec<PendingEvent>,
        run_id: &str,
        output: Option<RunOutputArchive>,
    ) -> Result<ControlSnapshot, ControlStoreError> {
        if self.rejects(&value) {
            return Err(ControlStoreError::Conflict);
        }
        self.inner
            .commit_terminal(revision, value, events, run_id, output)
            .await
    }

    async fn replay(
        &self,
        after: u64,
        limit: usize,
    ) -> Result<Vec<DurableEvent>, ControlStoreError> {
        self.inner.replay(after, limit).await
    }

    async fn event_bounds(&self) -> Result<EventBounds, ControlStoreError> {
        self.inner.event_bounds().await
    }

    async fn replay_page(
        &self,
        after: u64,
        limit: usize,
    ) -> Result<DurableEventPage, ControlStoreError> {
        self.inner.replay_page(after, limit).await
    }

    async fn save_progress(
        &self,
        checkpoint: ProgressCheckpoint,
        events: Vec<PendingEvent>,
    ) -> Result<(), ControlStoreError> {
        let delay = self.progress_delay_ms.load(Ordering::Relaxed);
        if delay > 0 {
            tokio::time::sleep(Duration::from_millis(delay)).await;
        }
        if self.fail_progress.load(Ordering::Relaxed) {
            return Err(ControlStoreError::Other(
                "injected progress persistence failure".into(),
            ));
        }
        self.inner.save_progress(checkpoint, events).await
    }

    async fn load_progress(&self) -> Result<Vec<ProgressCheckpoint>, ControlStoreError> {
        self.inner.load_progress().await
    }

    async fn load_run_outputs(
        &self,
        run_ids: &[String],
    ) -> Result<Vec<RunOutputArchive>, ControlStoreError> {
        self.inner.load_run_outputs(run_ids).await
    }

    async fn clear_progress(&self, run_id: &str) -> Result<(), ControlStoreError> {
        self.inner.clear_progress(run_id).await
    }
}

struct ArchiveReadFailingStore {
    inner: SqliteControlStore,
    output_reads: AtomicUsize,
    fail_on_output_read: AtomicUsize,
}

#[async_trait]
impl ControlStore for ArchiveReadFailingStore {
    async fn load(&self) -> Result<ControlSnapshot, ControlStoreError> {
        self.inner.load().await
    }

    async fn commit(
        &self,
        revision: u64,
        value: Value,
        events: Vec<PendingEvent>,
    ) -> Result<ControlSnapshot, ControlStoreError> {
        self.inner.commit(revision, value, events).await
    }

    async fn commit_terminal(
        &self,
        revision: u64,
        value: Value,
        events: Vec<PendingEvent>,
        run_id: &str,
        output: Option<RunOutputArchive>,
    ) -> Result<ControlSnapshot, ControlStoreError> {
        self.inner
            .commit_terminal(revision, value, events, run_id, output)
            .await
    }

    async fn replay(
        &self,
        after: u64,
        limit: usize,
    ) -> Result<Vec<DurableEvent>, ControlStoreError> {
        self.inner.replay(after, limit).await
    }

    async fn event_bounds(&self) -> Result<EventBounds, ControlStoreError> {
        self.inner.event_bounds().await
    }

    async fn replay_page(
        &self,
        after: u64,
        limit: usize,
    ) -> Result<DurableEventPage, ControlStoreError> {
        self.inner.replay_page(after, limit).await
    }

    async fn save_progress(
        &self,
        checkpoint: ProgressCheckpoint,
        events: Vec<PendingEvent>,
    ) -> Result<(), ControlStoreError> {
        self.inner.save_progress(checkpoint, events).await
    }

    async fn load_progress(&self) -> Result<Vec<ProgressCheckpoint>, ControlStoreError> {
        self.inner.load_progress().await
    }

    async fn load_run_outputs(
        &self,
        run_ids: &[String],
    ) -> Result<Vec<RunOutputArchive>, ControlStoreError> {
        let read = self.output_reads.fetch_add(1, Ordering::SeqCst) + 1;
        if read == self.fail_on_output_read.load(Ordering::SeqCst) {
            return Err(ControlStoreError::Other(
                "injected terminal archive read failure".into(),
            ));
        }
        self.inner.load_run_outputs(run_ids).await
    }

    async fn clear_progress(&self, run_id: &str) -> Result<(), ControlStoreError> {
        self.inner.clear_progress(run_id).await
    }
}

struct PausingProgressStore {
    inner: SqliteControlStore,
    pause_once: AtomicBool,
    save_started: Semaphore,
    allow_save: Semaphore,
    terminal_committed: Semaphore,
    progress_cleared: Semaphore,
}

#[async_trait]
impl ControlStore for PausingProgressStore {
    async fn load(&self) -> Result<ControlSnapshot, ControlStoreError> {
        self.inner.load().await
    }

    async fn commit(
        &self,
        revision: u64,
        value: Value,
        events: Vec<PendingEvent>,
    ) -> Result<ControlSnapshot, ControlStoreError> {
        let terminal = value["runs"]
            .as_array()
            .and_then(|runs| runs.last())
            .and_then(|run| run["status"].as_str())
            .is_some_and(|status| {
                matches!(
                    status,
                    "completed" | "failed" | "cancelled" | "limit_exceeded"
                )
            });
        let result = self.inner.commit(revision, value, events).await;
        if terminal && result.is_ok() {
            self.terminal_committed.add_permits(1);
        }
        result
    }

    async fn commit_terminal(
        &self,
        revision: u64,
        value: Value,
        events: Vec<PendingEvent>,
        run_id: &str,
        output: Option<RunOutputArchive>,
    ) -> Result<ControlSnapshot, ControlStoreError> {
        let result = self
            .inner
            .commit_terminal(revision, value, events, run_id, output)
            .await;
        if result.is_ok() {
            self.terminal_committed.add_permits(1);
            self.progress_cleared.add_permits(1);
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

    async fn event_bounds(&self) -> Result<EventBounds, ControlStoreError> {
        self.inner.event_bounds().await
    }

    async fn replay_page(
        &self,
        after: u64,
        limit: usize,
    ) -> Result<DurableEventPage, ControlStoreError> {
        self.inner.replay_page(after, limit).await
    }

    async fn save_progress(
        &self,
        checkpoint: ProgressCheckpoint,
        events: Vec<PendingEvent>,
    ) -> Result<(), ControlStoreError> {
        if self.pause_once.swap(false, Ordering::SeqCst) {
            self.save_started.add_permits(1);
            self.allow_save.acquire().await.unwrap().forget();
        }
        self.inner.save_progress(checkpoint, events).await
    }

    async fn load_progress(&self) -> Result<Vec<ProgressCheckpoint>, ControlStoreError> {
        self.inner.load_progress().await
    }

    async fn load_run_outputs(
        &self,
        run_ids: &[String],
    ) -> Result<Vec<RunOutputArchive>, ControlStoreError> {
        self.inner.load_run_outputs(run_ids).await
    }

    async fn clear_progress(&self, run_id: &str) -> Result<(), ControlStoreError> {
        let result = self.inner.clear_progress(run_id).await;
        if result.is_ok() {
            self.progress_cleared.add_permits(1);
        }
        result
    }
}

struct RecordingAgent {
    store: Arc<ConflictingStore>,
    calls: AtomicUsize,
    fail: AtomicBool,
}

#[async_trait]
impl WorkspaceAgent for RecordingAgent {
    async fn invoke(
        &self,
        request: WorkspaceAgentInvocation,
    ) -> Result<WorkspaceAgentResponse, DomainError> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        let snapshot = self.store.load().await.unwrap();
        let run = snapshot.value["runs"]
            .as_array()
            .unwrap()
            .iter()
            .find(|run| run["id"] == request.request_id)
            .unwrap();
        // Both the Run and its input must already be durable before the side effect.
        assert_eq!(run["status"], "running");
        assert!(
            snapshot.value["messages"]
                .as_array()
                .unwrap()
                .iter()
                .any(|message| message["id"] == run["base_message_id"])
        );
        if self.fail.load(Ordering::Relaxed) {
            return Err(DomainError::invariant(
                ErrorCode::ProviderFailed,
                "fixture failure",
            ));
        }
        Ok(WorkspaceAgentResponse {
            assistant_text: "fixture output".into(),
            commit_id: None,
            operations: Vec::new(),
            output_items: Vec::new(),
        })
    }
}

struct Fixture {
    _temporary: TempDir,
    store: Arc<ConflictingStore>,
    agent: Arc<RecordingAgent>,
    service: LocalControlService,
    project: ProjectView,
}

async fn command(service: &LocalControlService, command: Command) -> CommandResult {
    let response = service.execute(command).await;
    assert!(response.ok, "{:?}", response.error);
    response.result.unwrap()
}

async fn run(service: &LocalControlService, input: Command) -> RunView {
    match command(service, input).await {
        CommandResult::Run(run) => run,
        result => panic!("expected Run, got {result:?}"),
    }
}

fn send_message() -> Command {
    Command::SendMessage {
        session_id: "session".into(),
        text: "implement a feature".into(),
    }
}

impl Fixture {
    async fn new() -> Self {
        let temporary = TempDir::new().unwrap();
        let project_dir = temporary.path().join("project");
        std::fs::create_dir(&project_dir).unwrap();
        let store = Arc::new(ConflictingStore {
            inner: SqliteControlStore::in_memory().unwrap(),
            checkpoints: Mutex::default(),
            reject_runs: AtomicBool::new(false),
            progress_delay_ms: AtomicU64::new(0),
            fail_progress: AtomicBool::new(false),
        });
        let agent = Arc::new(RecordingAgent {
            store: store.clone(),
            calls: AtomicUsize::new(0),
            fail: AtomicBool::new(false),
        });
        let service = LocalControlService::with_workspace_agent(store.clone(), agent.clone());
        let CommandResult::Project(project) = command(
            &service,
            Command::RegisterProject {
                id: "project".into(),
                name: "Project".into(),
                workdir: project_dir.display().to_string(),
                repo_url: None,
            },
        )
        .await
        else {
            panic!("expected Project");
        };
        command(
            &service,
            Command::RegisterAgent {
                id: "agent".into(),
                name: "Agent".into(),
                config: config(),
            },
        )
        .await;
        command(
            &service,
            Command::CreateSession {
                id: "session".into(),
                project_id: project.id.clone(),
                agent_id: "agent".into(),
                at_message_id: None,
            },
        )
        .await;
        command(
            &service,
            Command::CreateCron {
                id: "cron".into(),
                name: "Cron".into(),
                project_id: project.id.clone(),
                base_message_id: project.root_message_id.clone(),
                agent_id: "agent".into(),
                schedule: "* * * * *".into(),
                timezone: "UTC".into(),
            },
        )
        .await;
        Self {
            _temporary: temporary,
            store,
            agent,
            service,
            project,
        }
    }
}

#[tokio::test]
async fn run_commands_return_persisted_terminal_results_and_do_not_repeat_external_calls_on_conflict()
 {
    let fixture = Fixture::new().await;
    for (index, input) in [
        send_message(),
        Command::ForkSession {
            id: "fork".into(),
            project_id: fixture.project.id.clone(),
            agent_id: "agent".into(),
            at_message_id: fixture.project.root_message_id.clone(),
            text: "implement a fork".into(),
        },
        Command::TriggerCron {
            cron_id: "cron".into(),
            scheduled_at: 42,
        },
    ]
    .into_iter()
    .enumerate()
    {
        *fixture.store.checkpoints.lock().unwrap() =
            VecDeque::from(["queued", "running", "completed"]);
        let result = run(&fixture.service, input).await;
        assert_eq!(result.status, "completed");
        assert!(result.error.is_none());
        assert!(result.last_message_id.is_some());
        assert_eq!(fixture.agent.calls.load(Ordering::Relaxed), index + 1);
        assert!(fixture.store.checkpoints.lock().unwrap().is_empty());
        let stored = run(
            &fixture.service,
            Command::GetRun {
                run_id: result.id.clone(),
            },
        )
        .await;
        assert_eq!(stored, result);
        let snapshot = fixture.store.load().await.unwrap();
        assert!(
            snapshot.value["sessions"]
                .as_array()
                .unwrap()
                .iter()
                .all(|session| session["active_run_id"].is_null())
        );
    }
    let replay = run(
        &fixture.service,
        Command::TriggerCron {
            cron_id: "cron".into(),
            scheduled_at: 42,
        },
    )
    .await;
    assert_eq!(replay.status, "completed");
    assert_eq!(fixture.agent.calls.load(Ordering::Relaxed), 3);
    assert_eq!(
        fixture.store.load().await.unwrap().value["runs"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
}

#[tokio::test]
async fn provider_failure_is_returned_as_a_persisted_failed_run() {
    let fixture = Fixture::new().await;
    fixture.agent.fail.store(true, Ordering::Relaxed);
    *fixture.store.checkpoints.lock().unwrap() = VecDeque::from(["failed"]);
    let result = run(&fixture.service, send_message()).await;
    assert_eq!(result.status, "failed");
    assert_eq!(
        result.error.as_ref().unwrap().code,
        ErrorCode::ProviderFailed
    );
    assert_eq!(fixture.agent.calls.load(Ordering::Relaxed), 1);
    assert!(fixture.store.checkpoints.lock().unwrap().is_empty());
    assert_eq!(
        run(
            &fixture.service,
            Command::GetRun {
                run_id: result.id.clone()
            }
        )
        .await,
        result
    );
    assert!(fixture.store.load().await.unwrap().value["sessions"][0]["active_run_id"].is_null());
}

#[tokio::test]
async fn failed_creation_commit_never_invokes_the_agent() {
    let fixture = Fixture::new().await;
    fixture.store.reject_runs.store(true, Ordering::Relaxed);
    let response = fixture.service.execute(send_message()).await;
    assert_eq!(response.error.unwrap().code, ErrorCode::RunQueueConflict);
    assert_eq!(fixture.agent.calls.load(Ordering::Relaxed), 0);
    let snapshot = fixture.store.load().await.unwrap();
    assert!(snapshot.value["runs"].as_array().unwrap().is_empty());
    assert_eq!(snapshot.value["messages"].as_array().unwrap().len(), 1);
    assert!(snapshot.value["sessions"][0]["active_run_id"].is_null());
}

#[tokio::test]
async fn post_admission_failures_queries_and_duplicate_cron_triggers_never_start_execution() {
    let fixture = Fixture::new().await;
    *fixture.store.checkpoints.lock().unwrap() =
        VecDeque::from(["running", "running", "running", "running"]);
    let interactive = run(&fixture.service, send_message()).await;
    assert_eq!(interactive.status, "failed");
    assert_eq!(
        interactive.error.as_ref().unwrap().code,
        ErrorCode::RunQueueConflict
    );
    let trigger = Command::TriggerCron {
        cron_id: "cron".into(),
        scheduled_at: 42,
    };
    *fixture.store.checkpoints.lock().unwrap() =
        VecDeque::from(["running", "running", "running", "running"]);
    let cron = run(&fixture.service, trigger.clone()).await;
    assert_eq!(cron.status, "failed");
    assert_eq!(
        cron.error.as_ref().unwrap().code,
        ErrorCode::RunQueueConflict
    );
    let before = fixture.store.load().await.unwrap();
    for terminal in [&interactive, &cron] {
        let result = run(
            &fixture.service,
            Command::GetRun {
                run_id: terminal.id.clone(),
            },
        )
        .await;
        assert_eq!(&result, terminal);
    }
    let after = fixture.store.load().await.unwrap();
    assert_eq!(before, after);
    assert_eq!(run(&fixture.service, trigger).await, cron);
    assert_eq!(fixture.agent.calls.load(Ordering::Relaxed), 0);
    assert_eq!(fixture.store.load().await.unwrap().value, before.value);
    let cancellation = fixture
        .service
        .execute(Command::CancelRun {
            run_id: interactive.id,
        })
        .await;
    assert_eq!(
        cancellation.error.unwrap().code,
        ErrorCode::RunAlreadyTerminal
    );
    assert!(fixture.store.load().await.unwrap().value["sessions"][0]["active_run_id"].is_null());
    assert_eq!(fixture.service.mark_interrupted_runs().await.unwrap(), 0);
    let recovered = run(&fixture.service, Command::GetRun { run_id: cron.id }).await;
    assert_eq!(recovered.status, "failed");
    assert_eq!(recovered.error.unwrap().code, ErrorCode::RunQueueConflict);
}

fn config() -> ait_contracts::AgentConfiguration {
    ait_contracts::AgentConfiguration {
        provider_id: "builtin-codex".into(),
        model: "gpt-5.6-sol".into(),
        reasoning_effort: Some("high".into()),
    }
}

struct SlowStreamingAgent {
    started: Semaphore,
    release: Semaphore,
}

struct PartialThenContinuingAgent {
    calls: AtomicUsize,
}

fn fixture_git_output(path: &std::path::Path, args: &[&str]) -> String {
    let output = ProcessCommand::new("git")
        .arg("-C")
        .arg(path)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

fn fixture_commit_all(path: &std::path::Path, subject: &str) {
    assert!(
        ProcessCommand::new("git")
            .arg("-C")
            .arg(path)
            .args(["add", "--all"])
            .status()
            .unwrap()
            .success()
    );
    assert!(
        ProcessCommand::new("git")
            .arg("-C")
            .arg(path)
            .args([
                "-c",
                "user.name=Ait Test",
                "-c",
                "user.email=ait-test@localhost",
                "commit",
                "--no-gpg-sign",
                "-m",
                subject,
            ])
            .status()
            .unwrap()
            .success()
    );
}

#[allow(
    clippy::too_many_lines,
    reason = "the recovery fixture models both retained failure and successful adoption"
)]
#[async_trait]
impl WorkspaceAgent for PartialThenContinuingAgent {
    async fn invoke(
        &self,
        _request: WorkspaceAgentInvocation,
    ) -> Result<WorkspaceAgentResponse, DomainError> {
        panic!("progress entry point expected")
    }

    async fn invoke_with_progress(
        &self,
        request: WorkspaceAgentInvocation,
        progress: Arc<dyn WorkspaceProgressReporter>,
    ) -> Result<WorkspaceAgentResponse, DomainError> {
        let call = self.calls.fetch_add(1, Ordering::Relaxed);
        if call == 0 {
            assert!(request.adopted_worktree.is_none());
            let identity = format!("{:x}", Sha256::digest(request.request_id.as_bytes()));
            let git_dir = fixture_git_output(&request.cwd, &["rev-parse", "--absolute-git-dir"]);
            let retained = std::path::PathBuf::from(git_dir)
                .join("ait")
                .join("workspaces")
                .join(&identity);
            let run_ref = format!("refs/ait/runs/{identity}");
            assert!(
                ProcessCommand::new("git")
                    .arg("-C")
                    .arg(&request.cwd)
                    .args(["update-ref", &run_ref, &request.baseline_commit])
                    .status()
                    .unwrap()
                    .success()
            );
            assert!(
                ProcessCommand::new("git")
                    .arg("-C")
                    .arg(&request.cwd)
                    .args([
                        "worktree",
                        "add",
                        "--detach",
                        retained.to_str().unwrap(),
                        &request.baseline_commit,
                    ])
                    .status()
                    .unwrap()
                    .success()
            );
            std::fs::write(retained.join("partial.txt"), "retained output\n").unwrap();
            progress
                .report(WorkspaceProgressEvent::MessageCompleted {
                    id: "commentary".into(),
                    phase: Some("commentary".into()),
                    text: "The file was written.".into(),
                })
                .await;
            progress
                .report(WorkspaceProgressEvent::OperationCompleted(
                    WorkspaceOperation {
                        id: "file-change".into(),
                        kind: "file_change".into(),
                        status: "completed".into(),
                        title: "Changed partial.txt".into(),
                        summary: None,
                        detail: None,
                        paths: vec!["partial.txt".into()],
                    },
                ))
                .await;
            progress
                .report(WorkspaceProgressEvent::MessageStarted {
                    id: "unfinished-final".into(),
                    phase: Some("final_answer".into()),
                    text: "This answer did not finish".into(),
                })
                .await;
            let mut failure =
                DomainError::invariant(ErrorCode::ProviderFailed, "fixture stream failure");
            failure.details = Some(ait_domain::DomainMetadata(BTreeMap::from([
                (
                    "retained_worktree_path".into(),
                    Value::String(retained.to_string_lossy().into_owned()),
                ),
                ("retained_run_ref".into(), Value::String(run_ref)),
                (
                    "retained_source_run_id".into(),
                    Value::String(request.request_id),
                ),
            ])));
            return Err(failure);
        }

        let adopted = request.adopted_worktree.expect("retained workspace");
        assert!(
            request
                .project_instructions
                .as_deref()
                .is_some_and(|value| value.contains("explicitly chose to continue"))
        );
        assert_eq!(
            std::fs::read_to_string(adopted.path.join("partial.txt")).unwrap(),
            "retained output\n"
        );
        fixture_commit_all(&adopted.path, "continue retained changes");
        let commit_id = fixture_git_output(&adopted.path, &["rev-parse", "HEAD"]);
        assert!(
            ProcessCommand::new("git")
                .arg("-C")
                .arg(&request.cwd)
                .args(["worktree", "remove", adopted.path.to_str().unwrap()])
                .status()
                .unwrap()
                .success()
        );
        assert!(
            ProcessCommand::new("git")
                .arg("-C")
                .arg(&request.cwd)
                .args(["merge", "--ff-only", &commit_id])
                .status()
                .unwrap()
                .success()
        );
        progress
            .report(WorkspaceProgressEvent::MessageCompleted {
                id: "continued-final".into(),
                phase: Some("final_answer".into()),
                text: "The retained work is complete.".into(),
            })
            .await;
        Ok(WorkspaceAgentResponse {
            assistant_text: "The retained work is complete.".into(),
            commit_id: Some(commit_id),
            operations: Vec::new(),
            output_items: vec![ait_ports::WorkspaceOutputItem::Message {
                id: "continued-final".into(),
                phase: Some("final_answer".into()),
                text: "The retained work is complete.".into(),
            }],
        })
    }
}

struct PanickingProgressAgent {
    calls: AtomicUsize,
    entered: Semaphore,
}

#[async_trait]
impl WorkspaceAgent for PanickingProgressAgent {
    async fn invoke(
        &self,
        _request: WorkspaceAgentInvocation,
    ) -> Result<WorkspaceAgentResponse, DomainError> {
        panic!("progress-aware entry point expected")
    }

    async fn invoke_with_progress(
        &self,
        _request: WorkspaceAgentInvocation,
        progress: Arc<dyn WorkspaceProgressReporter>,
    ) -> Result<WorkspaceAgentResponse, DomainError> {
        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        self.entered.add_permits(1);
        if call == 0 {
            progress
                .report(WorkspaceProgressEvent::MessageStarted {
                    id: "commentary".into(),
                    phase: Some("commentary".into()),
                    text: "before panic".into(),
                })
                .await;
            panic!("injected adapter panic after progress")
        }
        Ok(WorkspaceAgentResponse {
            assistant_text: "second run completed".into(),
            commit_id: None,
            operations: Vec::new(),
            output_items: Vec::new(),
        })
    }
}

#[tokio::test]
#[allow(
    clippy::too_many_lines,
    reason = "one recovery test exercises content, type, mode, and unchanged fingerprints"
)]
async fn terminal_partial_output_survives_reload_and_requires_exact_worktree_confirmation() {
    let temporary = TempDir::new().unwrap();
    let project_dir = temporary.path().join("partial-project");
    std::fs::create_dir(&project_dir).unwrap();
    let store = Arc::new(SqliteControlStore::in_memory().unwrap());
    let agent = Arc::new(PartialThenContinuingAgent {
        calls: AtomicUsize::new(0),
    });
    let service = LocalControlService::with_workspace_agent(store.clone(), agent.clone());
    let CommandResult::Project(project) = command(
        &service,
        Command::RegisterProject {
            id: "partial-project".into(),
            name: "Partial".into(),
            workdir: project_dir.display().to_string(),
            repo_url: None,
        },
    )
    .await
    else {
        panic!("expected Project")
    };
    command(
        &service,
        Command::RegisterAgent {
            id: "partial-agent".into(),
            name: "Partial agent".into(),
            config: config(),
        },
    )
    .await;
    command(
        &service,
        Command::CreateSession {
            id: "partial-session".into(),
            project_id: project.id,
            agent_id: "partial-agent".into(),
            at_message_id: None,
        },
    )
    .await;

    let failed = run(
        &service,
        Command::SendMessage {
            session_id: "partial-session".into(),
            text: "write then fail".into(),
        },
    )
    .await;
    assert_eq!(failed.status, "failed");
    assert!(failed.last_message_id.is_none());
    let partial = failed.partial_output.as_ref().expect("partial output");
    let progress = partial.progress.as_ref().expect("progress checkpoint");
    assert_eq!(progress["items"][0]["completed"], true);
    assert_eq!(progress["items"][2]["completed"], false);
    let worktree = partial.worktree.as_ref().expect("worktree state");
    assert!(worktree.dirty);
    assert_eq!(worktree.changes[0].path, "partial.txt");
    let retained_dir = std::path::PathBuf::from(
        worktree
            .retained_path
            .as_deref()
            .expect("isolated workspace path"),
    );
    assert_ne!(retained_dir, project_dir);
    assert!(!project_dir.join("partial.txt").exists());
    let fingerprint = worktree.fingerprint.clone();
    let persisted = store.load().await.unwrap();
    assert!(persisted.value["runs"][0].get("partial_output").is_none());

    let reopened = LocalControlService::with_workspace_agent(store.clone(), agent);
    let reloaded = run(
        &reopened,
        Command::GetRun {
            run_id: failed.id.clone(),
        },
    )
    .await;
    assert_eq!(reloaded.partial_output, failed.partial_output);

    std::fs::write(retained_dir.join("partial.txt"), "changed after failure\n").unwrap();
    let rejected = reopened
        .execute(Command::ContinueRun {
            run_id: failed.id.clone(),
            expected_worktree_fingerprint: fingerprint.clone(),
        })
        .await;
    assert_eq!(rejected.error.unwrap().code, ErrorCode::ProjectGitDirty);
    assert_eq!(agent_calls(&reopened, &failed.id).await, 0);

    std::fs::write(retained_dir.join("partial.txt"), "retained output\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::{PermissionsExt as _, symlink};

        let path = retained_dir.join("partial.txt");
        let original_mode = std::fs::metadata(&path).unwrap().permissions().mode();
        std::fs::remove_file(&path).unwrap();
        symlink("retained output\n", &path).unwrap();
        let rejected = reopened
            .execute(Command::ContinueRun {
                run_id: failed.id.clone(),
                expected_worktree_fingerprint: fingerprint.clone(),
            })
            .await;
        assert_eq!(rejected.error.unwrap().code, ErrorCode::ProjectGitDirty);

        std::fs::remove_file(&path).unwrap();
        std::fs::write(&path, "retained output\n").unwrap();
        let mut executable = std::fs::metadata(&path).unwrap().permissions();
        executable.set_mode(original_mode | 0o111);
        std::fs::set_permissions(&path, executable).unwrap();
        let rejected = reopened
            .execute(Command::ContinueRun {
                run_id: failed.id.clone(),
                expected_worktree_fingerprint: fingerprint.clone(),
            })
            .await;
        assert_eq!(rejected.error.unwrap().code, ErrorCode::ProjectGitDirty);

        let mut original = std::fs::metadata(&path).unwrap().permissions();
        original.set_mode(original_mode);
        std::fs::set_permissions(&path, original).unwrap();
    }
    let continued = run(
        &reopened,
        Command::ContinueRun {
            run_id: failed.id.clone(),
            expected_worktree_fingerprint: fingerprint,
        },
    )
    .await;
    assert_eq!(continued.status, "completed");
    assert_eq!(
        continued
            .recovery_of_run_id
            .as_ref()
            .map(ait_domain::RunId::as_str),
        Some(failed.id.as_str())
    );
    assert!(continued.last_message_id.is_some());
    let status = ProcessCommand::new("git")
        .arg("-C")
        .arg(project_dir)
        .args(["status", "--porcelain=v1"])
        .output()
        .unwrap();
    assert!(status.stdout.is_empty());
}

#[tokio::test]
#[allow(
    clippy::too_many_lines,
    reason = "the fault test proves admission, terminal state, Session release, and archive retention together"
)]
async fn admitted_recovery_archive_read_failure_terminalizes_and_preserves_the_source() {
    let temporary = TempDir::new().unwrap();
    let project_dir = temporary.path().join("archive-read-project");
    std::fs::create_dir(&project_dir).unwrap();
    let store = Arc::new(ArchiveReadFailingStore {
        inner: SqliteControlStore::in_memory().unwrap(),
        output_reads: AtomicUsize::new(0),
        fail_on_output_read: AtomicUsize::new(0),
    });
    let agent = Arc::new(PartialThenContinuingAgent {
        calls: AtomicUsize::new(0),
    });
    let service = LocalControlService::with_workspace_agent(store.clone(), agent.clone());
    let CommandResult::Project(project) = command(
        &service,
        Command::RegisterProject {
            id: "archive-read-project".into(),
            name: "Archive read".into(),
            workdir: project_dir.display().to_string(),
            repo_url: None,
        },
    )
    .await
    else {
        panic!("expected Project")
    };
    command(
        &service,
        Command::RegisterAgent {
            id: "archive-read-agent".into(),
            name: "Archive read agent".into(),
            config: config(),
        },
    )
    .await;
    command(
        &service,
        Command::CreateSession {
            id: "archive-read-session".into(),
            project_id: project.id,
            agent_id: "archive-read-agent".into(),
            at_message_id: None,
        },
    )
    .await;
    let source = run(
        &service,
        Command::SendMessage {
            session_id: "archive-read-session".into(),
            text: "retain work before the read fault".into(),
        },
    )
    .await;
    let fingerprint = source
        .partial_output
        .as_ref()
        .and_then(|output| output.worktree.as_ref())
        .expect("retained source worktree")
        .fingerprint
        .clone();

    store.output_reads.store(0, Ordering::SeqCst);
    store.fail_on_output_read.store(2, Ordering::SeqCst);
    let recovery = run(
        &service,
        Command::ContinueRun {
            run_id: source.id.clone(),
            expected_worktree_fingerprint: fingerprint,
        },
    )
    .await;

    assert_eq!(recovery.status, "failed");
    assert_eq!(
        recovery.error.as_ref().unwrap().code,
        ErrorCode::RunRecoveryFailed
    );
    assert!(
        recovery
            .error
            .as_ref()
            .unwrap()
            .message
            .contains("injected terminal archive read failure")
    );
    assert!(recovery.partial_output.is_none());
    assert_eq!(agent.calls.load(Ordering::SeqCst), 1);
    let CommandResult::Workspace(workspace) = command(&service, Command::Snapshot).await else {
        panic!("expected workspace")
    };
    assert!(
        workspace
            .sessions
            .iter()
            .find(|session| session.id == "archive-read-session")
            .unwrap()
            .active_run_id
            .is_none()
    );
    let source_archive = store
        .inner
        .load_run_outputs(std::slice::from_ref(&source.id))
        .await
        .unwrap()
        .pop()
        .expect("source archive remains available");
    assert!(
        source_archive
            .output
            .worktree
            .is_some_and(|worktree| worktree.dirty && worktree.retained_path.is_some())
    );
    assert!(source_archive.output.progress.is_some());
}

async fn agent_calls(service: &LocalControlService, source_run_id: &str) -> usize {
    let CommandResult::Workspace(workspace) = command(service, Command::Snapshot).await else {
        unreachable!()
    };
    workspace
        .runs
        .iter()
        .filter(|run| {
            run.recovery_of_run_id
                .as_ref()
                .is_some_and(|source| source.as_str() == source_run_id)
        })
        .count()
}

struct CancellablePartialAgent {
    started: Semaphore,
}

#[async_trait]
impl WorkspaceAgent for CancellablePartialAgent {
    async fn invoke(
        &self,
        _request: WorkspaceAgentInvocation,
    ) -> Result<WorkspaceAgentResponse, DomainError> {
        panic!("progress entry point expected")
    }

    async fn invoke_with_progress(
        &self,
        request: WorkspaceAgentInvocation,
        progress: Arc<dyn WorkspaceProgressReporter>,
    ) -> Result<WorkspaceAgentResponse, DomainError> {
        std::fs::write(request.cwd.join("cancelled.txt"), "kept\n").unwrap();
        progress
            .report(WorkspaceProgressEvent::MessageCompleted {
                id: "before-cancel".into(),
                phase: Some("commentary".into()),
                text: "Completed before cancellation.".into(),
            })
            .await;
        progress
            .report(WorkspaceProgressEvent::OperationCompleted(
                WorkspaceOperation {
                    id: "cancelled-file-change".into(),
                    kind: "file_change".into(),
                    status: "completed".into(),
                    title: "Changed cancelled.txt".into(),
                    summary: None,
                    detail: None,
                    paths: vec!["cancelled.txt".into()],
                },
            ))
            .await;
        progress
            .report(WorkspaceProgressEvent::MessageStarted {
                id: "during-cancel".into(),
                phase: Some("final_answer".into()),
                text: "Unfinished".into(),
            })
            .await;
        self.started.add_permits(1);
        request.cancellation.cancelled().await;
        Err(DomainError::invariant(
            ErrorCode::RunCancelled,
            "run was cancelled after partial output",
        ))
    }
}

#[tokio::test]
async fn cancellation_retains_confirmed_and_unfinished_output_with_workspace_state() {
    let temporary = TempDir::new().unwrap();
    let project_dir = temporary.path().join("cancel-project");
    std::fs::create_dir(&project_dir).unwrap();
    let store = Arc::new(SqliteControlStore::in_memory().unwrap());
    let agent = Arc::new(CancellablePartialAgent {
        started: Semaphore::new(0),
    });
    let service = Arc::new(LocalControlService::with_workspace_agent(
        store,
        agent.clone(),
    ));
    let CommandResult::Project(project) = command(
        &service,
        Command::RegisterProject {
            id: "cancel-project".into(),
            name: "Cancel".into(),
            workdir: project_dir.display().to_string(),
            repo_url: None,
        },
    )
    .await
    else {
        panic!("expected Project")
    };
    command(
        &service,
        Command::RegisterAgent {
            id: "cancel-agent".into(),
            name: "Cancel agent".into(),
            config: config(),
        },
    )
    .await;
    command(
        &service,
        Command::CreateSession {
            id: "cancel-session".into(),
            project_id: project.id,
            agent_id: "cancel-agent".into(),
            at_message_id: None,
        },
    )
    .await;
    let executing = {
        let service = service.clone();
        tokio::spawn(async move {
            run(
                &service,
                Command::SendMessage {
                    session_id: "cancel-session".into(),
                    text: "write then wait".into(),
                },
            )
            .await
        })
    };
    tokio::time::timeout(Duration::from_secs(2), agent.started.acquire())
        .await
        .unwrap()
        .unwrap()
        .forget();
    let CommandResult::Workspace(workspace) = command(&service, Command::Snapshot).await else {
        unreachable!()
    };
    let run_id = workspace.runs[0].id.clone();
    command(
        &service,
        Command::CancelRun {
            run_id: run_id.clone(),
        },
    )
    .await;
    let cancelled = tokio::time::timeout(Duration::from_secs(2), executing)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(cancelled.status, "cancelled");
    assert!(cancelled.last_message_id.is_none());
    let partial = cancelled.partial_output.expect("partial output");
    assert_eq!(
        partial.progress.as_ref().unwrap()["items"][0]["completed"],
        true
    );
    assert_eq!(
        partial.progress.as_ref().unwrap()["items"][1]["operation"]["status"],
        "completed"
    );
    assert_eq!(
        partial.progress.as_ref().unwrap()["items"][2]["completed"],
        false
    );
    assert!(partial.worktree.as_ref().is_some_and(|state| state.dirty));
    assert_eq!(partial.worktree.unwrap().changes[0].path, "cancelled.txt");
    assert!(service.progress_checkpoints().await.unwrap().is_empty());
}

struct CompletesAsCancellationCommitsAgent {
    started: Semaphore,
    release: Semaphore,
}

#[async_trait]
impl WorkspaceAgent for CompletesAsCancellationCommitsAgent {
    async fn invoke(
        &self,
        _request: WorkspaceAgentInvocation,
    ) -> Result<WorkspaceAgentResponse, DomainError> {
        panic!("progress entry point expected")
    }

    async fn invoke_with_progress(
        &self,
        _request: WorkspaceAgentInvocation,
        progress: Arc<dyn WorkspaceProgressReporter>,
    ) -> Result<WorkspaceAgentResponse, DomainError> {
        progress
            .report(WorkspaceProgressEvent::MessageCompleted {
                id: "racing-final".into(),
                phase: Some("final_answer".into()),
                text: "Completed while cancellation committed.".into(),
            })
            .await;
        self.started.add_permits(1);
        self.release.acquire().await.unwrap().forget();
        Ok(WorkspaceAgentResponse {
            assistant_text: "Completed while cancellation committed.".into(),
            commit_id: Some("committed-before-cancel".into()),
            operations: Vec::new(),
            output_items: vec![ait_ports::WorkspaceOutputItem::Message {
                id: "racing-final".into(),
                phase: Some("final_answer".into()),
                text: "Completed while cancellation committed.".into(),
            }],
        })
    }
}

#[tokio::test]
async fn cancellation_winning_the_terminal_cas_archives_a_concurrent_success() {
    let temporary = TempDir::new().unwrap();
    let project_dir = temporary.path().join("racing-project");
    std::fs::create_dir(&project_dir).unwrap();
    let store = Arc::new(SqliteControlStore::in_memory().unwrap());
    let agent = Arc::new(CompletesAsCancellationCommitsAgent {
        started: Semaphore::new(0),
        release: Semaphore::new(0),
    });
    let service = Arc::new(LocalControlService::with_workspace_agent(
        store,
        agent.clone(),
    ));
    let CommandResult::Project(project) = command(
        &service,
        Command::RegisterProject {
            id: "racing-project".into(),
            name: "Racing".into(),
            workdir: project_dir.display().to_string(),
            repo_url: None,
        },
    )
    .await
    else {
        panic!("expected Project")
    };
    command(
        &service,
        Command::RegisterAgent {
            id: "racing-agent".into(),
            name: "Racing agent".into(),
            config: config(),
        },
    )
    .await;
    command(
        &service,
        Command::CreateSession {
            id: "racing-session".into(),
            project_id: project.id,
            agent_id: "racing-agent".into(),
            at_message_id: None,
        },
    )
    .await;

    let executing = {
        let service = service.clone();
        tokio::spawn(async move {
            run(
                &service,
                Command::SendMessage {
                    session_id: "racing-session".into(),
                    text: "finish as cancellation lands".into(),
                },
            )
            .await
        })
    };
    tokio::time::timeout(Duration::from_secs(2), agent.started.acquire())
        .await
        .unwrap()
        .unwrap()
        .forget();
    let CommandResult::Workspace(workspace) = command(&service, Command::Snapshot).await else {
        unreachable!()
    };
    let run_id = workspace.runs[0].id.clone();
    command(&service, Command::CancelRun { run_id }).await;
    agent.release.add_permits(1);
    let cancelled = tokio::time::timeout(Duration::from_secs(2), executing)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(cancelled.status, "cancelled");
    assert!(cancelled.last_message_id.is_none());
    let progress = cancelled
        .partial_output
        .expect("successful output must be archived when cancellation wins")
        .progress
        .expect("final progress checkpoint");
    assert_eq!(progress["items"][0]["completed"], true);
    assert_eq!(
        progress["items"][0]["text"],
        "Completed while cancellation committed."
    );
}

struct FailingProgressAgent;

#[async_trait]
impl WorkspaceAgent for FailingProgressAgent {
    async fn invoke(
        &self,
        _request: WorkspaceAgentInvocation,
    ) -> Result<WorkspaceAgentResponse, DomainError> {
        panic!("progress entry point expected")
    }

    async fn invoke_with_progress(
        &self,
        _request: WorkspaceAgentInvocation,
        progress: Arc<dyn WorkspaceProgressReporter>,
    ) -> Result<WorkspaceAgentResponse, DomainError> {
        progress
            .report(WorkspaceProgressEvent::MessageCompleted {
                id: "checkpoint".into(),
                phase: Some("commentary".into()),
                text: "must survive terminal archival".into(),
            })
            .await;
        Err(DomainError::invariant(
            ErrorCode::ProviderFailed,
            "injected provider failure",
        ))
    }
}

struct BreaksWorktreeInspectionAgent;

#[async_trait]
impl WorkspaceAgent for BreaksWorktreeInspectionAgent {
    async fn invoke(
        &self,
        _request: WorkspaceAgentInvocation,
    ) -> Result<WorkspaceAgentResponse, DomainError> {
        panic!("progress entry point expected")
    }

    async fn invoke_with_progress(
        &self,
        request: WorkspaceAgentInvocation,
        progress: Arc<dyn WorkspaceProgressReporter>,
    ) -> Result<WorkspaceAgentResponse, DomainError> {
        progress
            .report(WorkspaceProgressEvent::MessageCompleted {
                id: "before-inspection-failure".into(),
                phase: Some("commentary".into()),
                text: "Git metadata is about to disappear.".into(),
            })
            .await;
        std::fs::rename(request.cwd.join(".git"), request.cwd.join("git-hidden")).unwrap();
        Err(DomainError::invariant(
            ErrorCode::ProviderFailed,
            "injected provider failure",
        ))
    }
}

#[tokio::test]
async fn terminal_archive_persists_an_explicit_unknown_worktree_state() {
    let temporary = TempDir::new().unwrap();
    let project_dir = temporary.path().join("unknown-worktree-project");
    std::fs::create_dir(&project_dir).unwrap();
    let store = Arc::new(SqliteControlStore::in_memory().unwrap());
    let service = LocalControlService::with_workspace_agent(
        store.clone(),
        Arc::new(BreaksWorktreeInspectionAgent),
    );
    let CommandResult::Project(project) = command(
        &service,
        Command::RegisterProject {
            id: "unknown-worktree-project".into(),
            name: "Unknown worktree".into(),
            workdir: project_dir.display().to_string(),
            repo_url: None,
        },
    )
    .await
    else {
        panic!("expected Project")
    };
    command(
        &service,
        Command::RegisterAgent {
            id: "unknown-worktree-agent".into(),
            name: "Unknown worktree agent".into(),
            config: config(),
        },
    )
    .await;
    command(
        &service,
        Command::CreateSession {
            id: "unknown-worktree-session".into(),
            project_id: project.id,
            agent_id: "unknown-worktree-agent".into(),
            at_message_id: None,
        },
    )
    .await;

    let failed = run(
        &service,
        Command::SendMessage {
            session_id: "unknown-worktree-session".into(),
            text: "break inspection".into(),
        },
    )
    .await;
    let partial = failed.partial_output.unwrap();
    assert!(partial.worktree.is_none());
    assert_eq!(
        partial.worktree_error.as_ref().unwrap().code,
        ErrorCode::ProjectGitHeadUnavailable
    );

    let reopened = LocalControlService::new(store);
    let reloaded = run(&reopened, Command::GetRun { run_id: failed.id }).await;
    assert!(reloaded.partial_output.unwrap().worktree_error.is_some());
}

#[tokio::test]
async fn terminal_archival_waits_for_slow_progress_and_survives_progress_store_failure() {
    let temporary = TempDir::new().unwrap();
    let project_dir = temporary.path().join("progress-project");
    std::fs::create_dir(&project_dir).unwrap();
    let store = Arc::new(ConflictingStore {
        inner: SqliteControlStore::in_memory().unwrap(),
        checkpoints: Mutex::default(),
        reject_runs: AtomicBool::new(false),
        progress_delay_ms: AtomicU64::new(600),
        fail_progress: AtomicBool::new(false),
    });
    let service =
        LocalControlService::with_workspace_agent(store.clone(), Arc::new(FailingProgressAgent));
    let CommandResult::Project(project) = command(
        &service,
        Command::RegisterProject {
            id: "progress-project".into(),
            name: "Progress".into(),
            workdir: project_dir.display().to_string(),
            repo_url: None,
        },
    )
    .await
    else {
        panic!("expected Project")
    };
    command(
        &service,
        Command::RegisterAgent {
            id: "progress-agent".into(),
            name: "Progress agent".into(),
            config: config(),
        },
    )
    .await;
    command(
        &service,
        Command::CreateSession {
            id: "slow-progress-session".into(),
            project_id: project.id.clone(),
            agent_id: "progress-agent".into(),
            at_message_id: None,
        },
    )
    .await;

    let started = std::time::Instant::now();
    let delayed = run(
        &service,
        Command::SendMessage {
            session_id: "slow-progress-session".into(),
            text: "slow checkpoint".into(),
        },
    )
    .await;
    assert!(started.elapsed() >= Duration::from_secs(1));
    assert_eq!(
        delayed.partial_output.unwrap().progress.unwrap()["items"][0]["text"],
        "must survive terminal archival"
    );

    store.progress_delay_ms.store(0, Ordering::Relaxed);
    store.fail_progress.store(true, Ordering::Relaxed);
    command(
        &service,
        Command::CreateSession {
            id: "failed-progress-session".into(),
            project_id: project.id,
            agent_id: "progress-agent".into(),
            at_message_id: None,
        },
    )
    .await;
    let failed_store = run(
        &service,
        Command::SendMessage {
            session_id: "failed-progress-session".into(),
            text: "failed checkpoint store".into(),
        },
    )
    .await;
    let partial = failed_store.partial_output.unwrap();
    let checkpoint = partial.progress.unwrap();
    assert_eq!(
        checkpoint["items"][0]["text"],
        "must survive terminal archival"
    );
    assert_eq!(
        partial.progress_error.unwrap().message,
        "injected progress persistence failure"
    );
}

#[async_trait]
impl WorkspaceAgent for SlowStreamingAgent {
    async fn invoke(
        &self,
        _request: WorkspaceAgentInvocation,
    ) -> Result<WorkspaceAgentResponse, DomainError> {
        panic!("streaming entry point expected")
    }

    async fn invoke_with_progress(
        &self,
        _request: WorkspaceAgentInvocation,
        progress: Arc<dyn WorkspaceProgressReporter>,
    ) -> Result<WorkspaceAgentResponse, DomainError> {
        progress
            .report(WorkspaceProgressEvent::MessageStarted {
                id: "commentary".into(),
                phase: Some("commentary".into()),
                text: String::new(),
            })
            .await;
        for index in 0..300 {
            progress
                .report(WorkspaceProgressEvent::TextDelta {
                    id: "commentary".into(),
                    delta: format!("{index} "),
                })
                .await;
        }
        progress
            .report(WorkspaceProgressEvent::OperationStarted(
                WorkspaceOperation {
                    id: "tool".into(),
                    kind: "read".into(),
                    status: "inProgress".into(),
                    title: "Read file".into(),
                    summary: Some("src/main.rs".into()),
                    detail: None,
                    paths: vec!["src/main.rs".into()],
                },
            ))
            .await;
        self.started.add_permits(1);
        self.release.acquire().await.unwrap().forget();
        progress
            .report(WorkspaceProgressEvent::MessageCompleted {
                id: "final".into(),
                phase: Some("final_answer".into()),
                text: "done".into(),
            })
            .await;
        progress
            .report(WorkspaceProgressEvent::TurnStatus {
                status: "completed".into(),
                error: None,
            })
            .await;
        Ok(WorkspaceAgentResponse {
            assistant_text: "done".into(),
            commit_id: None,
            operations: Vec::new(),
            output_items: vec![ait_ports::WorkspaceOutputItem::Message {
                id: "final".into(),
                phase: Some("final_answer".into()),
                text: "done".into(),
            }],
        })
    }
}

#[tokio::test]
#[allow(
    clippy::too_many_lines,
    reason = "one vertical-slice test keeps concurrent execution and replay assertions together"
)]
async fn asynchronous_submission_streams_batched_progress_and_survives_replay_pagination() {
    let temporary = TempDir::new().unwrap();
    let project_dir = temporary.path().join("project");
    let second_project_dir = temporary.path().join("project-2");
    std::fs::create_dir(&project_dir).unwrap();
    std::fs::create_dir(&second_project_dir).unwrap();
    let store = Arc::new(SqliteControlStore::in_memory().unwrap());
    let agent = Arc::new(SlowStreamingAgent {
        started: Semaphore::new(0),
        release: Semaphore::new(0),
    });
    let service = Arc::new(LocalControlService::with_workspace_agent(
        store.clone(),
        agent.clone(),
    ));
    let CommandResult::Project(project) = command(
        &service,
        Command::RegisterProject {
            id: "live-project".into(),
            name: "Live".into(),
            workdir: project_dir.display().to_string(),
            repo_url: None,
        },
    )
    .await
    else {
        panic!("expected project")
    };
    command(
        &service,
        Command::RegisterAgent {
            id: "live-agent".into(),
            name: "Live agent".into(),
            config: config(),
        },
    )
    .await;
    command(
        &service,
        Command::CreateSession {
            id: "live-session".into(),
            project_id: project.id,
            agent_id: "live-agent".into(),
            at_message_id: None,
        },
    )
    .await;

    let accepted = tokio::time::timeout(
        Duration::from_millis(500),
        service.submit(Command::SendMessage {
            session_id: "live-session".into(),
            text: "stream it".into(),
        }),
    )
    .await
    .expect("submission must not wait for the turn");
    let CommandResult::Run(run) = accepted.result.unwrap() else {
        panic!("expected accepted run")
    };
    assert_eq!(run.status, "queued");
    tokio::time::timeout(Duration::from_secs(2), agent.started.acquire())
        .await
        .unwrap()
        .unwrap()
        .forget();
    tokio::time::sleep(Duration::from_millis(100)).await;

    let checkpoints = service.progress_checkpoints().await.unwrap();
    assert_eq!(checkpoints.len(), 1);
    assert_eq!(checkpoints[0]["run_id"], run.id);
    assert_eq!(checkpoints[0]["seq"], 302);
    assert_eq!(
        checkpoints[0]["items"][1]["operation"]["status"],
        "inProgress"
    );
    let first_page = service.event_page(0, 128).await.unwrap();
    assert_eq!(first_page.events.len(), 128);
    let next_page = service
        .event_page(first_page.events.last().unwrap().cursor, 256)
        .await
        .unwrap();
    assert!(next_page.cursor_valid);
    assert!(next_page.events.len() > 170);
    assert!(
        next_page
            .events
            .iter()
            .filter(|event| event.kind == "run.progress")
            .map(|event| event.body["seq"].as_u64().unwrap())
            .is_sorted()
    );

    command(
        &service,
        Command::RegisterProject {
            id: "live-project-2".into(),
            name: "Live 2".into(),
            workdir: second_project_dir.display().to_string(),
            repo_url: None,
        },
    )
    .await;
    command(
        &service,
        Command::CreateSession {
            id: "live-session-2".into(),
            project_id: "live-project-2".into(),
            agent_id: "live-agent".into(),
            at_message_id: None,
        },
    )
    .await;
    let second = service
        .submit(Command::SendMessage {
            session_id: "live-session-2".into(),
            text: "stream separately".into(),
        })
        .await;
    let CommandResult::Run(second) = second.result.unwrap() else {
        panic!("expected second accepted run")
    };
    tokio::time::timeout(Duration::from_secs(2), agent.started.acquire())
        .await
        .unwrap()
        .unwrap()
        .forget();
    tokio::time::sleep(Duration::from_millis(100)).await;
    let concurrent = service.progress_checkpoints().await.unwrap();
    assert_eq!(concurrent.len(), 2);
    assert!(
        concurrent
            .iter()
            .any(|value| { value["run_id"] == run.id && value["session_id"] == "live-session" })
    );
    assert!(
        concurrent.iter().any(|value| {
            value["run_id"] == second.id && value["session_id"] == "live-session-2"
        })
    );

    agent.release.add_permits(2);
    let completed = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let CommandResult::Run(run) = command(
                &service,
                Command::GetRun {
                    run_id: run.id.clone(),
                },
            )
            .await
            else {
                unreachable!()
            };
            if run.status == "completed" {
                break run;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(completed.last_message_id.is_some());
    let second_completed = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let CommandResult::Run(current) = command(
                &service,
                Command::GetRun {
                    run_id: second.id.clone(),
                },
            )
            .await
            else {
                unreachable!()
            };
            if current.status == "completed" {
                break current;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(second_completed.last_message_id.is_some());
    assert!(service.progress_checkpoints().await.unwrap().is_empty());
    assert!(!service.event_page(u64::MAX, 10).await.unwrap().cursor_valid);
}

#[tokio::test]
#[allow(
    clippy::too_many_lines,
    reason = "the deterministic panic ordering test keeps every lifecycle assertion in one fixture"
)]
async fn provider_panic_drains_progress_before_terminal_cleanup_and_releases_leases() {
    let temporary = TempDir::new().unwrap();
    let project_dir = temporary.path().join("panic-project");
    std::fs::create_dir(&project_dir).unwrap();
    let store = Arc::new(PausingProgressStore {
        inner: SqliteControlStore::in_memory().unwrap(),
        pause_once: AtomicBool::new(true),
        save_started: Semaphore::new(0),
        allow_save: Semaphore::new(0),
        terminal_committed: Semaphore::new(0),
        progress_cleared: Semaphore::new(0),
    });
    let agent = Arc::new(PanickingProgressAgent {
        calls: AtomicUsize::new(0),
        entered: Semaphore::new(0),
    });
    let service = Arc::new(LocalControlService::with_workspace_agent(
        store.clone(),
        agent.clone(),
    ));
    let CommandResult::Project(project) = command(
        &service,
        Command::RegisterProject {
            id: "panic-project".into(),
            name: "Panic project".into(),
            workdir: project_dir.display().to_string(),
            repo_url: None,
        },
    )
    .await
    else {
        panic!("expected Project")
    };
    command(
        &service,
        Command::RegisterAgent {
            id: "panic-agent".into(),
            name: "Panic agent".into(),
            config: config(),
        },
    )
    .await;
    for session_id in ["panic-session", "next-session"] {
        command(
            &service,
            Command::CreateSession {
                id: session_id.into(),
                project_id: project.id.clone(),
                agent_id: "panic-agent".into(),
                at_message_id: None,
            },
        )
        .await;
    }

    let accepted = service
        .submit(Command::SendMessage {
            session_id: "panic-session".into(),
            text: "panic after progress".into(),
        })
        .await;
    let CommandResult::Run(first) = accepted.result.unwrap() else {
        panic!("expected accepted Run")
    };
    assert_eq!(first.status, "queued");
    tokio::time::timeout(Duration::from_secs(2), agent.entered.acquire())
        .await
        .unwrap()
        .unwrap()
        .forget();
    tokio::time::timeout(Duration::from_secs(2), store.save_started.acquire())
        .await
        .unwrap()
        .unwrap()
        .forget();

    let CommandResult::Workspace(pending) = command(&service, Command::Snapshot).await else {
        panic!("expected workspace")
    };
    assert_eq!(
        pending
            .runs
            .iter()
            .find(|run| run.id == first.id)
            .unwrap()
            .status,
        "running"
    );
    assert_eq!(
        pending
            .sessions
            .iter()
            .find(|session| session.id == "panic-session")
            .unwrap()
            .active_run_id
            .as_deref(),
        Some(first.id.as_str())
    );
    let next_submission = {
        let service = service.clone();
        tokio::spawn(async move {
            service
                .submit(Command::SendMessage {
                    session_id: "next-session".into(),
                    text: "run after panic".into(),
                })
                .await
        })
    };
    assert!(
        tokio::time::timeout(Duration::from_millis(100), agent.entered.acquire())
            .await
            .is_err(),
        "same-Project execution entered before progress drain and terminal persistence"
    );

    store.allow_save.add_permits(1);
    tokio::time::timeout(Duration::from_secs(2), store.terminal_committed.acquire())
        .await
        .unwrap()
        .unwrap()
        .forget();
    tokio::time::timeout(Duration::from_secs(2), store.progress_cleared.acquire())
        .await
        .unwrap()
        .unwrap()
        .forget();

    let CommandResult::Workspace(settled) = command(&service, Command::Snapshot).await else {
        panic!("expected workspace")
    };
    let failed = settled.runs.iter().find(|run| run.id == first.id).unwrap();
    assert_eq!(failed.status, "failed");
    assert_eq!(
        failed.error.as_ref().unwrap().code,
        ErrorCode::ProviderFailed
    );
    assert!(
        settled
            .sessions
            .iter()
            .find(|session| session.id == "panic-session")
            .unwrap()
            .active_run_id
            .is_none()
    );
    assert!(
        store
            .load_progress()
            .await
            .unwrap()
            .iter()
            .all(|checkpoint| checkpoint.run_id != first.id)
    );
    let run_events = store
        .replay(0, 100)
        .await
        .unwrap()
        .into_iter()
        .filter(|event| event.entity_id.as_deref() == Some(first.id.as_str()))
        .collect::<Vec<_>>();
    let terminal = run_events
        .iter()
        .position(|event| event.kind == "run.updated" && event.body["status"] == "failed")
        .expect("failed terminal event");
    assert!(run_events[terminal].body.get("partial_output").is_none());
    assert!(
        serde_json::to_vec(&run_events[terminal].body)
            .unwrap()
            .len()
            < 1024
    );
    assert!(
        run_events[..terminal]
            .iter()
            .any(|event| event.kind == "run.progress")
    );
    assert!(
        run_events[terminal + 1..]
            .iter()
            .all(|event| event.kind != "run.progress")
    );
    assert_eq!(
        run_events.last().unwrap().cursor,
        run_events[terminal].cursor
    );

    let next = tokio::time::timeout(Duration::from_secs(2), next_submission)
        .await
        .unwrap()
        .unwrap();
    let CommandResult::Run(next) = next.result.unwrap() else {
        panic!("expected next accepted Run")
    };
    tokio::time::timeout(Duration::from_secs(2), agent.entered.acquire())
        .await
        .unwrap()
        .unwrap()
        .forget();
    tokio::time::timeout(Duration::from_secs(2), store.terminal_committed.acquire())
        .await
        .unwrap()
        .unwrap()
        .forget();
    tokio::time::timeout(Duration::from_secs(2), store.progress_cleared.acquire())
        .await
        .unwrap()
        .unwrap()
        .forget();
    let CommandResult::Run(completed) = command(
        &service,
        Command::GetRun {
            run_id: next.id.clone(),
        },
    )
    .await
    else {
        panic!("expected Run")
    };
    assert_eq!(completed.status, "completed");
    assert!(store.load_progress().await.unwrap().is_empty());
}
