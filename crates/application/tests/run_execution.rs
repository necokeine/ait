//! Command execution, durable checkpoints, and read-only Run queries.

mod support;

use std::{
    collections::VecDeque,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};

use ait_application::LocalControlService;
use ait_contracts::{Command, CommandResult, ProjectView, RunView};
use ait_domain::{DomainError, ErrorCode};
use ait_ports::{
    ControlChange, ControlFilter, ControlRead, ControlStore, ControlStoreError, DurableEvent,
    DurableEventPage, EventBounds, PendingEvent, ProgressCheckpoint, WorkspaceAgent,
    WorkspaceAgentInvocation, WorkspaceAgentResponse, WorkspaceOperation, WorkspaceProgressEvent,
    WorkspaceProgressReporter,
};
use ait_storage_sqlite::SqliteControlStore;
use async_trait::async_trait;
use serde_json::Value;
use tempfile::TempDir;
use tokio::{sync::Semaphore, time::Duration};

use support::{ControlStoreTestExt, terminal_run_status, workspace};

struct ConflictingStore {
    inner: SqliteControlStore,
    checkpoints: Mutex<VecDeque<&'static str>>,
    reject_runs: AtomicBool,
}

#[async_trait]
impl ControlStore for ConflictingStore {
    async fn read(&self, filters: &[ControlFilter]) -> Result<ControlRead, ControlStoreError> {
        self.inner.read(filters).await
    }

    async fn apply(
        &self,
        revision: u64,
        changes: Vec<ControlChange>,
        events: Vec<PendingEvent>,
    ) -> Result<u64, ControlStoreError> {
        let status = terminal_run_status(&changes);
        let conflict = {
            let mut checkpoints = self.checkpoints.lock().unwrap();
            if status.is_some() && status == checkpoints.front().copied() {
                checkpoints.pop_front();
                true
            } else {
                status.is_some() && self.reject_runs.load(Ordering::Relaxed)
            }
        };
        if conflict {
            return Err(ControlStoreError::Conflict);
        }
        self.inner.apply(revision, changes, events).await
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

    async fn load_progress(
        &self,
        project_id: &str,
    ) -> Result<Vec<ProgressCheckpoint>, ControlStoreError> {
        self.inner.load_progress(project_id).await
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
    async fn read(&self, filters: &[ControlFilter]) -> Result<ControlRead, ControlStoreError> {
        self.inner.read(filters).await
    }

    async fn apply(
        &self,
        revision: u64,
        changes: Vec<ControlChange>,
        events: Vec<PendingEvent>,
    ) -> Result<u64, ControlStoreError> {
        let terminal = terminal_run_status(&changes).is_some_and(|status| {
            matches!(
                status,
                "completed" | "failed" | "cancelled" | "limit_exceeded"
            )
        });
        let result = self.inner.apply(revision, changes, events).await;
        if terminal && result.is_ok() {
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

    async fn load_progress(
        &self,
        project_id: &str,
    ) -> Result<Vec<ProgressCheckpoint>, ControlStoreError> {
        self.inner.load_progress(project_id).await
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
    recoveries: AtomicUsize,
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

    async fn recover_checkpointed(
        &self,
        _request: WorkspaceAgentInvocation,
        result: WorkspaceAgentResponse,
        _baseline_ref: Option<String>,
    ) -> Result<WorkspaceAgentResponse, DomainError> {
        self.recoveries.fetch_add(1, Ordering::Relaxed);
        Ok(result)
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
        });
        let agent = Arc::new(RecordingAgent {
            store: store.clone(),
            calls: AtomicUsize::new(0),
            recoveries: AtomicUsize::new(0),
            fail: AtomicBool::new(false),
        });
        let service = LocalControlService::with_workspace_agent(store.clone(), agent.clone());
        let CommandResult::Project(project) = command(
            &service,
            Command::RegisterProject {
                id: "project".into(),
                name: "Project".into(),
                workdir: Some(project_dir.display().to_string()),
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
    assert!(
        fixture
            .service
            .recover_interrupted_runs()
            .await
            .unwrap()
            .is_empty()
    );
    let recovered = run(&fixture.service, Command::GetRun { run_id: cron.id }).await;
    assert_eq!(recovered.status, "failed");
    assert_eq!(recovered.error.unwrap().code, ErrorCode::RunQueueConflict);
}

#[tokio::test]
async fn startup_recovery_executes_a_queued_run_once_without_query_side_effects() {
    let fixture = Fixture::new().await;
    let completed = run(&fixture.service, send_message()).await;
    rewind_completed_run(&fixture.store, &completed, "queued").await;
    let service =
        LocalControlService::with_workspace_agent(fixture.store.clone(), fixture.agent.clone());

    let before = fixture.agent.calls.load(Ordering::Relaxed);
    let queried = run(
        &service,
        Command::GetRun {
            run_id: completed.id.clone(),
        },
    )
    .await;
    assert_eq!(queried.status, "queued");
    assert_eq!(fixture.agent.calls.load(Ordering::Relaxed), before);

    let recovered = service.recover_interrupted_runs().await.unwrap();
    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0].status, "completed");
    assert_eq!(fixture.agent.calls.load(Ordering::Relaxed), before + 1);
    assert_eq!(fixture.agent.recoveries.load(Ordering::Relaxed), 0);
}

#[tokio::test]
async fn startup_recovery_finalizes_a_checkpoint_without_reinvoking_the_agent() {
    let fixture = Fixture::new().await;
    let completed = run(&fixture.service, send_message()).await;
    rewind_completed_run(&fixture.store, &completed, "settling").await;
    let service =
        LocalControlService::with_workspace_agent(fixture.store.clone(), fixture.agent.clone());
    let calls = fixture.agent.calls.load(Ordering::Relaxed);

    let recovered = service.recover_interrupted_runs().await.unwrap();
    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0].status, "completed");
    assert_eq!(fixture.agent.calls.load(Ordering::Relaxed), calls);
    assert_eq!(fixture.agent.recoveries.load(Ordering::Relaxed), 1);
    let snapshot = fixture.store.load().await.unwrap();
    let run_messages = snapshot.value["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|message| {
            message["parent_message_id"] == completed.base_message_id
                && message["role"] == "assistant"
        })
        .count();
    // The completed fixture's historical assistant Message is immutable; recovery
    // appends exactly one finalized Message beside it.
    assert_eq!(run_messages, 2);
}

#[tokio::test]
async fn startup_recovery_interrupts_unknown_running_effects_and_releases_the_session() {
    let fixture = Fixture::new().await;
    let completed = run(&fixture.service, send_message()).await;
    rewind_completed_run(&fixture.store, &completed, "running").await;
    let service =
        LocalControlService::with_workspace_agent(fixture.store.clone(), fixture.agent.clone());
    let calls = fixture.agent.calls.load(Ordering::Relaxed);

    let recovered = service.recover_interrupted_runs().await.unwrap();
    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0].status, "interrupted");
    assert_eq!(
        recovered[0].error.as_ref().unwrap().code,
        ErrorCode::RunRecoveryFailed
    );
    assert_eq!(fixture.agent.calls.load(Ordering::Relaxed), calls);
    assert_eq!(fixture.agent.recoveries.load(Ordering::Relaxed), 0);
    assert!(fixture.store.load().await.unwrap().value["sessions"][0]["active_run_id"].is_null());
}

#[tokio::test]
async fn ask_and_fail_recovery_policies_never_replay_queued_work() {
    for (policy, expected_status) in [("ask", "interrupted"), ("fail", "failed")] {
        let fixture = Fixture::new().await;
        let completed = run(&fixture.service, send_message()).await;
        rewind_completed_run(&fixture.store, &completed, "queued").await;
        let snapshot = fixture.store.load().await.unwrap();
        let mut value = snapshot.value;
        value["settings"]["runtime.recovery"] = Value::String(policy.into());
        fixture
            .store
            .commit(snapshot.revision, value, Vec::new())
            .await
            .unwrap();
        let calls = fixture.agent.calls.load(Ordering::Relaxed);
        let service =
            LocalControlService::with_workspace_agent(fixture.store.clone(), fixture.agent.clone());

        let recovered = service.recover_interrupted_runs().await.unwrap();
        assert_eq!(recovered.len(), 1);
        assert_eq!(recovered[0].status, expected_status);
        assert_eq!(fixture.agent.calls.load(Ordering::Relaxed), calls);
        assert!(
            fixture.store.load().await.unwrap().value["sessions"][0]["active_run_id"].is_null()
        );
    }
}

async fn rewind_completed_run(store: &ConflictingStore, completed: &RunView, status: &str) {
    let snapshot = store.load().await.unwrap();
    let mut value = snapshot.value;
    let run = value["runs"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|run| run["id"] == completed.id)
        .unwrap();
    run["status"] = Value::String(status.into());
    run["phase"] = Value::String(if status == "settling" {
        "result_persisted".into()
    } else {
        status.into()
    });
    run["last_message_id"] = Value::Null;
    run["error"] = Value::Null;
    value["sessions"][0]["active_run_id"] = Value::String(completed.id.clone());
    value["sessions"][0]["current_message_id"] = Value::String(completed.base_message_id.clone());
    store
        .commit(snapshot.revision, value, Vec::new())
        .await
        .unwrap();
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
            workdir: Some(project_dir.display().to_string()),
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

    let checkpoints = service.progress_checkpoints("live-project").await.unwrap();
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
            workdir: Some(second_project_dir.display().to_string()),
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
    let first_project = service.progress_checkpoints("live-project").await.unwrap();
    assert_eq!(first_project.len(), 1);
    assert_eq!(first_project[0]["run_id"], run.id);
    let second_project = service
        .progress_checkpoints("live-project-2")
        .await
        .unwrap();
    assert_eq!(second_project.len(), 1);
    assert_eq!(second_project[0]["run_id"], second.id);

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
    assert!(
        service
            .progress_checkpoints("live-project")
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        service
            .progress_checkpoints("live-project-2")
            .await
            .unwrap()
            .is_empty()
    );
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
            workdir: Some(project_dir.display().to_string()),
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

    let pending = workspace(&service).await;
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

    let settled = workspace(&service).await;
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
            .load_progress("panic-project")
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
    assert!(
        store
            .load_progress("panic-project")
            .await
            .unwrap()
            .is_empty()
    );
}
