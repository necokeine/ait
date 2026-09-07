//! Command execution, durable checkpoints, and read-only Run queries.

use std::{
    collections::VecDeque,
    process::Command as ProcessCommand,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};

use ait_application::LocalControlService;
use ait_contracts::{Command, CommandResult, ProjectView, RunView};
use ait_domain::{DomainError, ErrorCode};
use ait_ports::{
    ControlSnapshot, ControlStore, ControlStoreError, DurableEvent, DurableEventPage, EventBounds,
    PendingEvent, ProgressCheckpoint, WorkspaceAgent, WorkspaceAgentInvocation,
    WorkspaceAgentResponse, WorkspaceOperation, WorkspaceProgressEvent, WorkspaceProgressReporter,
};
use ait_storage_sqlite::SqliteControlStore;
use async_trait::async_trait;
use serde_json::Value;
use tempfile::TempDir;
use tokio::{sync::Semaphore, time::Duration};

struct ConflictingStore {
    inner: SqliteControlStore,
    checkpoints: Mutex<VecDeque<&'static str>>,
    reject_runs: AtomicBool,
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
        let status = value["runs"]
            .as_array()
            .and_then(|runs| runs.last())
            .and_then(|run| run["status"].as_str());
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
        self.inner.commit(revision, value, events).await
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

    async fn clear_progress(&self, run_id: &str) -> Result<(), ControlStoreError> {
        self.inner.clear_progress(run_id).await
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
async fn queued_run_queries_and_duplicate_cron_triggers_never_start_execution() {
    let fixture = Fixture::new().await;
    *fixture.store.checkpoints.lock().unwrap() =
        VecDeque::from(["running", "running", "running", "running"]);
    let response = fixture.service.execute(send_message()).await;
    assert_eq!(response.error.unwrap().code, ErrorCode::RunQueueConflict);
    let snapshot = fixture.store.load().await.unwrap();
    let interactive: RunView = serde_json::from_value(snapshot.value["runs"][0].clone()).unwrap();
    assert_eq!(interactive.status, "queued");
    let trigger = Command::TriggerCron {
        cron_id: "cron".into(),
        scheduled_at: 42,
    };
    *fixture.store.checkpoints.lock().unwrap() =
        VecDeque::from(["running", "running", "running", "running"]);
    let response = fixture.service.execute(trigger.clone()).await;
    assert_eq!(response.error.unwrap().code, ErrorCode::RunQueueConflict);
    let snapshot = fixture.store.load().await.unwrap();
    let cron: RunView = serde_json::from_value(snapshot.value["runs"][1].clone()).unwrap();
    assert_eq!(cron.status, "queued");
    let before = fixture.store.load().await.unwrap();
    for queued in [&interactive, &cron] {
        let result = run(
            &fixture.service,
            Command::GetRun {
                run_id: queued.id.clone(),
            },
        )
        .await;
        assert_eq!(&result, queued);
    }
    let after = fixture.store.load().await.unwrap();
    assert_eq!(before, after);
    assert_eq!(run(&fixture.service, trigger).await, cron);
    assert_eq!(fixture.agent.calls.load(Ordering::Relaxed), 0);
    assert_eq!(fixture.store.load().await.unwrap().value, before.value);
    let cancelled = run(
        &fixture.service,
        Command::CancelRun {
            run_id: interactive.id,
        },
    )
    .await;
    assert_eq!(cancelled.status, "cancelled");
    assert!(fixture.store.load().await.unwrap().value["sessions"][0]["active_run_id"].is_null());
    assert_eq!(fixture.service.mark_interrupted_runs().await.unwrap(), 1);
    let recovered = run(&fixture.service, Command::GetRun { run_id: cron.id }).await;
    assert_eq!(recovered.status, "failed");
    assert_eq!(recovered.error.unwrap().code, ErrorCode::RunRecoveryFailed);
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
            assert!(request.adopted_worktree_fingerprint.is_none());
            std::fs::write(request.cwd.join("partial.txt"), "retained output\n").unwrap();
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
            progress
                .report(WorkspaceProgressEvent::TurnStatus {
                    status: "failed".into(),
                    error: Some("fixture stream failure".into()),
                })
                .await;
            return Err(DomainError::invariant(
                ErrorCode::ProviderFailed,
                "fixture stream failure",
            ));
        }

        assert!(request.adopted_worktree_fingerprint.is_some());
        assert!(
            request
                .project_instructions
                .as_deref()
                .is_some_and(|value| value.contains("explicitly chose to continue"))
        );
        assert_eq!(
            std::fs::read_to_string(request.cwd.join("partial.txt")).unwrap(),
            "retained output\n"
        );
        let added = ProcessCommand::new("git")
            .arg("-C")
            .arg(&request.cwd)
            .args(["add", "--all"])
            .status()
            .unwrap();
        assert!(added.success());
        let committed = ProcessCommand::new("git")
            .arg("-C")
            .arg(&request.cwd)
            .args([
                "-c",
                "user.name=Ait Test",
                "-c",
                "user.email=ait-test@localhost",
                "commit",
                "--no-gpg-sign",
                "-m",
                "continue retained changes",
            ])
            .status()
            .unwrap();
        assert!(committed.success());
        progress
            .report(WorkspaceProgressEvent::MessageCompleted {
                id: "continued-final".into(),
                phase: Some("final_answer".into()),
                text: "The retained work is complete.".into(),
            })
            .await;
        Ok(WorkspaceAgentResponse {
            assistant_text: "The retained work is complete.".into(),
            commit_id: None,
            operations: Vec::new(),
            output_items: vec![ait_ports::WorkspaceOutputItem::Message {
                id: "continued-final".into(),
                phase: Some("final_answer".into()),
                text: "The retained work is complete.".into(),
            }],
        })
    }
}

#[tokio::test]
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
    let fingerprint = worktree.fingerprint.clone();

    let reopened = LocalControlService::with_workspace_agent(store.clone(), agent);
    let reloaded = run(
        &reopened,
        Command::GetRun {
            run_id: failed.id.clone(),
        },
    )
    .await;
    assert_eq!(reloaded.partial_output, failed.partial_output);

    std::fs::write(project_dir.join("partial.txt"), "changed after failure\n").unwrap();
    let rejected = reopened
        .execute(Command::ContinueRun {
            run_id: failed.id.clone(),
            expected_worktree_fingerprint: fingerprint.clone(),
        })
        .await;
    assert_eq!(rejected.error.unwrap().code, ErrorCode::ProjectGitDirty);
    assert_eq!(agent_calls(&reopened, &failed.id).await, 0);

    std::fs::write(project_dir.join("partial.txt"), "retained output\n").unwrap();
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
        continued.recovery_of_run_id.as_deref(),
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

async fn agent_calls(service: &LocalControlService, source_run_id: &str) -> usize {
    let CommandResult::Workspace(workspace) = command(service, Command::Snapshot).await else {
        unreachable!()
    };
    workspace
        .runs
        .iter()
        .filter(|run| run.recovery_of_run_id.as_deref() == Some(source_run_id))
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
        std::future::pending::<()>().await;
        unreachable!()
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
    std::fs::create_dir(&project_dir).unwrap();
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
        Command::CreateSession {
            id: "live-session-2".into(),
            project_id: "live-project".into(),
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
