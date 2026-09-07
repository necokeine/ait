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
    ControlSnapshot, ControlStore, ControlStoreError, DurableEvent, PendingEvent, WorkspaceAgent,
    WorkspaceAgentInvocation, WorkspaceAgentResponse, WorkspaceResultSink,
};
use ait_storage_sqlite::SqliteControlStore;
use async_trait::async_trait;
use serde_json::Value;
use tempfile::TempDir;
use tokio::sync::Notify;

struct ConflictingStore {
    inner: SqliteControlStore,
    checkpoints: Mutex<VecDeque<&'static str>>,
    failures: Mutex<VecDeque<&'static str>>,
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
            let mut failures = self.failures.lock().unwrap();
            if status.is_some() && status == failures.front().copied() {
                failures.pop_front();
                return Err(ControlStoreError::Other(
                    "injected durable store failure".into(),
                ));
            }
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
}

struct RecordingAgent {
    store: Arc<ConflictingStore>,
    calls: AtomicUsize,
    fail: AtomicBool,
    edit_workspace: AtomicBool,
    stop_after_checkpoint: AtomicBool,
}

struct SerialAgent {
    calls: AtomicUsize,
    active: AtomicUsize,
    max_active: AtomicUsize,
    first_started: Notify,
    release_first: Notify,
}

#[async_trait]
impl WorkspaceAgent for SerialAgent {
    async fn invoke(
        &self,
        request: WorkspaceAgentInvocation,
    ) -> Result<WorkspaceAgentResponse, DomainError> {
        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        self.max_active.fetch_max(active, Ordering::SeqCst);
        if call == 0 {
            self.first_started.notify_one();
            self.release_first.notified().await;
        }
        std::fs::write(
            request.cwd.join(format!("serial-{call}.txt")),
            format!("call {call}\n"),
        )
        .unwrap();
        self.active.fetch_sub(1, Ordering::SeqCst);
        Ok(WorkspaceAgentResponse {
            assistant_text: format!("serial output {call}"),
            operations: Vec::new(),
            output_items: Vec::new(),
        })
    }
}

struct FencingAgent {
    checkpointed: Notify,
    release: Notify,
    late_error: Mutex<Option<DomainError>>,
}

#[async_trait]
impl WorkspaceAgent for FencingAgent {
    async fn invoke(
        &self,
        _request: WorkspaceAgentInvocation,
    ) -> Result<WorkspaceAgentResponse, DomainError> {
        unreachable!("fencing fixture uses invoke_and_checkpoint")
    }

    async fn invoke_and_checkpoint(
        &self,
        request: WorkspaceAgentInvocation,
        result_sink: &dyn WorkspaceResultSink,
    ) -> Result<WorkspaceAgentResponse, DomainError> {
        std::fs::write(request.cwd.join("fenced.txt"), "must not commit\n").unwrap();
        let result = WorkspaceAgentResponse {
            assistant_text: "checkpointed output".into(),
            operations: Vec::new(),
            output_items: Vec::new(),
        };
        result_sink.checkpoint(result.clone()).await?;
        self.checkpointed.notify_one();
        self.release.notified().await;
        let failure = result_sink.checkpoint(result.clone()).await.unwrap_err();
        *self.late_error.lock().unwrap() = Some(failure.clone());
        Err(failure)
    }
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
        if self.edit_workspace.load(Ordering::Relaxed) {
            std::fs::write(request.cwd.join("answer.txt"), "generated once\n").unwrap();
        }
        Ok(WorkspaceAgentResponse {
            assistant_text: "fixture output".into(),
            operations: Vec::new(),
            output_items: Vec::new(),
        })
    }

    async fn invoke_and_checkpoint(
        &self,
        request: WorkspaceAgentInvocation,
        result_sink: &dyn WorkspaceResultSink,
    ) -> Result<WorkspaceAgentResponse, DomainError> {
        let result = self.invoke(request).await?;
        result_sink.checkpoint(result.clone()).await?;
        if self.stop_after_checkpoint.load(Ordering::Relaxed) {
            return Err(DomainError::invariant(
                ErrorCode::RunRecoveryFailed,
                "simulated daemon crash after result checkpoint",
            ));
        }
        Ok(result)
    }
}

struct Fixture {
    temporary: TempDir,
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
            failures: Mutex::default(),
            reject_runs: AtomicBool::new(false),
        });
        let agent = Arc::new(RecordingAgent {
            store: store.clone(),
            calls: AtomicUsize::new(0),
            fail: AtomicBool::new(false),
            edit_workspace: AtomicBool::new(false),
            stop_after_checkpoint: AtomicBool::new(false),
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
            temporary,
            store,
            agent,
            service,
            project,
        }
    }

    fn restarted_service(&self) -> LocalControlService {
        LocalControlService::with_workspace_agent(self.store.clone(), self.agent.clone())
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
}

fn config() -> ait_contracts::AgentConfiguration {
    ait_contracts::AgentConfiguration {
        provider_id: "builtin-codex".into(),
        model: "gpt-5.6-sol".into(),
        reasoning_effort: Some("high".into()),
    }
}

#[tokio::test]
async fn resume_safe_starts_a_durable_queued_run_but_queries_do_not() {
    let fixture = Fixture::new().await;
    *fixture.store.checkpoints.lock().unwrap() =
        VecDeque::from(["running", "running", "running", "running"]);
    let response = fixture.service.execute(send_message()).await;
    assert_eq!(response.error.unwrap().code, ErrorCode::RunQueueConflict);
    assert_eq!(fixture.agent.calls.load(Ordering::Relaxed), 0);
    let queued: RunView =
        serde_json::from_value(fixture.store.load().await.unwrap().value["runs"][0].clone())
            .unwrap();
    assert_eq!(queued.status, "queued");

    let queried = run(
        &fixture.service,
        Command::GetRun {
            run_id: queued.id.clone(),
        },
    )
    .await;
    assert_eq!(queried.status, "queued");
    assert_eq!(fixture.agent.calls.load(Ordering::Relaxed), 0);

    let restarted = fixture.restarted_service();
    let recovered = restarted.recover_interrupted_runs().await.unwrap();
    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0].status, "completed");
    assert_eq!(recovered[0].operation_id, queued.operation_id);
    assert_eq!(recovered[0].lease_epoch, 2);
    assert_eq!(fixture.agent.calls.load(Ordering::Relaxed), 1);
    assert!(fixture.store.load().await.unwrap().value["sessions"][0]["active_run_id"].is_null());
}

#[tokio::test]
async fn unknown_running_effects_are_interrupted_without_replay_and_release_the_session() {
    let fixture = Fixture::new().await;
    *fixture.store.checkpoints.lock().unwrap() =
        VecDeque::from(["running", "running", "running", "running"]);
    let response = fixture.service.execute(send_message()).await;
    assert_eq!(response.error.unwrap().code, ErrorCode::RunQueueConflict);

    let snapshot = fixture.store.load().await.unwrap();
    let mut value = snapshot.value;
    let run_id = value["runs"][0]["id"].as_str().unwrap().to_owned();
    let operation_id = value["runs"][0]["operation_id"]
        .as_str()
        .unwrap()
        .to_owned();
    value["runs"][0]["status"] = serde_json::json!("running");
    value["runs"][0]["phase"] = serde_json::json!("calling_agent");
    value["runs"][0]["lease_epoch"] = serde_json::json!(1);
    value["workspace_run_journals"][&run_id] = serde_json::json!({
        "operation_id": operation_id,
        "lease_epoch": 1,
        "commit_subject": "implement a feature",
        "expected_head": fixture.project.base_commit,
        "observed_head": null,
        "result": null,
        "commit_id": null,
        "settled": false,
    });
    fixture
        .store
        .commit(snapshot.revision, value, Vec::new())
        .await
        .unwrap();

    let restarted = fixture.restarted_service();
    let recovered = restarted.recover_interrupted_runs().await.unwrap();
    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0].status, "interrupted");
    assert_eq!(
        recovered[0].error.as_ref().unwrap().code,
        ErrorCode::RunRecoveryFailed
    );
    assert_eq!(fixture.agent.calls.load(Ordering::Relaxed), 0);
    assert!(fixture.store.load().await.unwrap().value["sessions"][0]["active_run_id"].is_null());
    assert!(
        restarted
            .recover_interrupted_runs()
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(fixture.agent.calls.load(Ordering::Relaxed), 0);
}

#[tokio::test]
async fn completed_agent_result_is_recovered_before_git_without_reinvocation() {
    let fixture = Fixture::new().await;
    fixture.agent.edit_workspace.store(true, Ordering::Relaxed);
    fixture
        .agent
        .stop_after_checkpoint
        .store(true, Ordering::Relaxed);

    let response = fixture.service.execute(send_message()).await;
    assert_eq!(response.error.unwrap().code, ErrorCode::RunRecoveryFailed);
    let snapshot = fixture.store.load().await.unwrap();
    let pending: RunView = serde_json::from_value(snapshot.value["runs"][0].clone()).unwrap();
    assert_eq!(pending.status, "finalizing");
    assert_eq!(pending.phase.as_deref(), Some("result_persisted"));
    assert_eq!(fixture.agent.calls.load(Ordering::Relaxed), 1);
    assert_eq!(
        git(&fixture.project.workdir, &["rev-parse", "HEAD"]),
        fixture.project.base_commit
    );
    assert!(
        !git(&fixture.project.workdir, &["status", "--porcelain=v1"]).is_empty(),
        "the uncommitted Agent change must be preserved"
    );

    let restarted = fixture.restarted_service();
    let recovered = restarted.recover_interrupted_runs().await.unwrap();
    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0].status, "completed");
    assert_eq!(recovered[0].lease_epoch, 2);
    assert_eq!(fixture.agent.calls.load(Ordering::Relaxed), 1);
    assert!(git(&fixture.project.workdir, &["status", "--porcelain=v1"]).is_empty());
    assert_ne!(
        git(&fixture.project.workdir, &["rev-parse", "HEAD"]),
        fixture.project.base_commit
    );
}

#[tokio::test]
async fn recovery_preserves_a_diverged_head_for_user_review() {
    let fixture = Fixture::new().await;
    fixture.agent.edit_workspace.store(true, Ordering::Relaxed);
    fixture
        .agent
        .stop_after_checkpoint
        .store(true, Ordering::Relaxed);
    let response = fixture.service.execute(send_message()).await;
    assert_eq!(response.error.unwrap().code, ErrorCode::RunRecoveryFailed);

    let output = ProcessCommand::new("git")
        .arg("-C")
        .arg(&fixture.project.workdir)
        .args([
            "-c",
            "user.name=Fixture User",
            "-c",
            "user.email=fixture@example.invalid",
            "add",
            "--all",
        ])
        .output()
        .unwrap();
    assert!(output.status.success());
    let output = ProcessCommand::new("git")
        .arg("-C")
        .arg(&fixture.project.workdir)
        .args([
            "-c",
            "user.name=Fixture User",
            "-c",
            "user.email=fixture@example.invalid",
            "commit",
            "--no-gpg-sign",
            "-m",
            "user-owned settlement",
        ])
        .output()
        .unwrap();
    assert!(output.status.success());
    let user_head = git(&fixture.project.workdir, &["rev-parse", "HEAD"]);

    let restarted = fixture.restarted_service();
    let recovered = restarted.recover_interrupted_runs().await.unwrap();
    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0].status, "interrupted");
    assert_eq!(recovered[0].lease_epoch, 2);
    assert_eq!(
        git(&fixture.project.workdir, &["rev-parse", "HEAD"]),
        user_head
    );
    assert_eq!(fixture.agent.calls.load(Ordering::Relaxed), 1);
    let snapshot = fixture.store.load().await.unwrap();
    assert!(snapshot.value["sessions"][0]["active_run_id"].is_null());
    assert_eq!(
        snapshot.value["messages"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|message| message["role"] == "assistant")
            .count(),
        0
    );
}

#[tokio::test]
async fn committed_git_result_is_settled_once_after_conflict_or_store_failure() {
    for fail_with_conflicts in [true, false] {
        let fixture = Fixture::new().await;
        fixture.agent.edit_workspace.store(true, Ordering::Relaxed);
        if fail_with_conflicts {
            *fixture.store.checkpoints.lock().unwrap() =
                VecDeque::from(["completed", "completed", "completed", "completed"]);
        } else {
            *fixture.store.failures.lock().unwrap() = VecDeque::from(["completed"]);
        }

        let response = fixture.service.execute(send_message()).await;
        assert!(!response.ok);
        let snapshot = fixture.store.load().await.unwrap();
        let pending: RunView = serde_json::from_value(snapshot.value["runs"][0].clone()).unwrap();
        assert_eq!(pending.status, "finalizing");
        assert_eq!(pending.phase.as_deref(), Some("result_persisted"));
        assert_eq!(fixture.agent.calls.load(Ordering::Relaxed), 1);
        assert_eq!(
            snapshot.value["messages"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|message| message["role"] == "assistant")
                .count(),
            0
        );
        let head_before_recovery = git(&fixture.project.workdir, &["rev-parse", "HEAD"]);
        let commit_body = git(
            &fixture.project.workdir,
            &["show", "-s", "--format=%B", "HEAD"],
        );
        assert!(commit_body.contains(pending.operation_id.as_deref().unwrap()));

        let restarted = fixture.restarted_service();
        let recovered = restarted.recover_interrupted_runs().await.unwrap();
        assert_eq!(recovered.len(), 1);
        assert_eq!(recovered[0].status, "completed");
        assert_eq!(recovered[0].lease_epoch, 2);
        assert_eq!(fixture.agent.calls.load(Ordering::Relaxed), 1);
        assert_eq!(
            git(&fixture.project.workdir, &["rev-parse", "HEAD"]),
            head_before_recovery
        );
        let snapshot = fixture.store.load().await.unwrap();
        assert!(snapshot.value["sessions"][0]["active_run_id"].is_null());
        assert_eq!(
            snapshot.value["messages"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|message| message["role"] == "assistant")
                .count(),
            1
        );
        assert!(
            restarted
                .recover_interrupted_runs()
                .await
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            git(&fixture.project.workdir, &["rev-parse", "HEAD"]),
            head_before_recovery
        );
    }
}

#[tokio::test]
async fn ask_and_fail_recovery_policies_never_replay_queued_work() {
    for (policy, expected_status) in [("ask", "interrupted"), ("fail", "failed")] {
        let fixture = Fixture::new().await;
        *fixture.store.checkpoints.lock().unwrap() =
            VecDeque::from(["running", "running", "running", "running"]);
        let response = fixture.service.execute(send_message()).await;
        assert_eq!(response.error.unwrap().code, ErrorCode::RunQueueConflict);
        let CommandResult::Settings(mut settings) =
            command(&fixture.service, Command::GetSettings).await
        else {
            panic!("expected settings");
        };
        settings
            .values
            .0
            .insert("runtime.recovery".into(), serde_json::json!(policy));
        command(
            &fixture.service,
            Command::SaveSettings {
                expected_revision: settings.revision,
                values: settings.values,
            },
        )
        .await;

        let restarted = fixture.restarted_service();
        let recovered = restarted.recover_interrupted_runs().await.unwrap();
        assert_eq!(recovered.len(), 1);
        assert_eq!(recovered[0].status, expected_status);
        assert_eq!(fixture.agent.calls.load(Ordering::Relaxed), 0);
        assert!(
            fixture.store.load().await.unwrap().value["sessions"][0]["active_run_id"].is_null()
        );
    }
}

#[tokio::test]
async fn startup_scan_becomes_query_ready_before_a_blocked_recovered_agent_finishes() {
    let fixture = Fixture::new().await;
    *fixture.store.checkpoints.lock().unwrap() =
        VecDeque::from(["running", "running", "running", "running"]);
    let response = fixture.service.execute(send_message()).await;
    assert_eq!(response.error.unwrap().code, ErrorCode::RunQueueConflict);

    let agent = Arc::new(SerialAgent {
        calls: AtomicUsize::new(0),
        active: AtomicUsize::new(0),
        max_active: AtomicUsize::new(0),
        first_started: Notify::new(),
        release_first: Notify::new(),
    });
    let restarted = Arc::new(LocalControlService::with_workspace_agent(
        fixture.store.clone(),
        agent.clone(),
    ));
    let plan = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        restarted.prepare_startup_recovery(),
    )
    .await
    .expect("startup scan must not wait for Agent execution")
    .unwrap();
    assert_eq!(plan.len(), 1);
    assert_eq!(agent.calls.load(Ordering::SeqCst), 0);

    let supervisor = {
        let restarted = restarted.clone();
        tokio::spawn(async move { restarted.run_startup_recovery(plan).await })
    };
    tokio::time::timeout(
        std::time::Duration::from_secs(1),
        agent.first_started.notified(),
    )
    .await
    .expect("recovery supervisor should start the queued Run");
    let running = run(
        &restarted,
        Command::GetRun {
            run_id: fixture.store.load().await.unwrap().value["runs"][0]["id"]
                .as_str()
                .unwrap()
                .to_owned(),
        },
    )
    .await;
    assert_eq!(running.status, "running");
    assert!(!supervisor.is_finished());
    agent.release_first.notify_one();
    let recovered = supervisor.await.unwrap().unwrap();
    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0].status, "completed");
    assert_eq!(agent.calls.load(Ordering::SeqCst), 1);
    assert!(
        restarted
            .recover_interrupted_runs()
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn checkpointed_operation_rejects_later_user_worktree_and_index_changes_without_committing() {
    let fixture = Fixture::new().await;
    fixture.agent.edit_workspace.store(true, Ordering::Relaxed);
    fixture
        .agent
        .stop_after_checkpoint
        .store(true, Ordering::Relaxed);
    let response = fixture.service.execute(send_message()).await;
    assert_eq!(response.error.unwrap().code, ErrorCode::RunRecoveryFailed);

    std::fs::write(
        std::path::Path::new(&fixture.project.workdir).join("user.txt"),
        "user-owned change\n",
    )
    .unwrap();
    git(&fixture.project.workdir, &["add", "user.txt"]);
    let head = git(&fixture.project.workdir, &["rev-parse", "HEAD"]);

    let recovered = fixture
        .restarted_service()
        .recover_interrupted_runs()
        .await
        .unwrap();
    assert_eq!(recovered[0].status, "interrupted");
    assert_eq!(git(&fixture.project.workdir, &["rev-parse", "HEAD"]), head);
    assert_eq!(
        git(
            &fixture.project.workdir,
            &["diff", "--cached", "--name-only"]
        ),
        "user.txt"
    );
    assert!(
        git(&fixture.project.workdir, &["status", "--porcelain=v1"])
            .lines()
            .any(|line| line.ends_with("answer.txt")),
        "Ait must not stage the checkpointed Agent file after a mismatch"
    );
}

#[tokio::test]
async fn same_project_workspace_runs_are_serialized_from_git_snapshot_through_commit() {
    let temporary = TempDir::new().unwrap();
    let project_dir = temporary.path().join("project");
    std::fs::create_dir(&project_dir).unwrap();
    let store = Arc::new(SqliteControlStore::in_memory().unwrap());
    let agent = Arc::new(SerialAgent {
        calls: AtomicUsize::new(0),
        active: AtomicUsize::new(0),
        max_active: AtomicUsize::new(0),
        first_started: Notify::new(),
        release_first: Notify::new(),
    });
    let service = Arc::new(LocalControlService::with_workspace_agent(
        store,
        agent.clone(),
    ));
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
    for session_id in ["session-1", "session-2"] {
        command(
            &service,
            Command::CreateSession {
                id: session_id.into(),
                project_id: project.id.clone(),
                agent_id: "agent".into(),
                at_message_id: None,
            },
        )
        .await;
    }

    let first = {
        let service = service.clone();
        tokio::spawn(async move {
            service
                .execute(Command::SendMessage {
                    session_id: "session-1".into(),
                    text: "first".into(),
                })
                .await
        })
    };
    tokio::time::timeout(
        std::time::Duration::from_secs(1),
        agent.first_started.notified(),
    )
    .await
    .unwrap();
    let second = {
        let service = service.clone();
        tokio::spawn(async move {
            service
                .execute(Command::SendMessage {
                    session_id: "session-2".into(),
                    text: "second".into(),
                })
                .await
        })
    };
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert_eq!(agent.calls.load(Ordering::SeqCst), 1);
    assert_eq!(agent.max_active.load(Ordering::SeqCst), 1);
    agent.release_first.notify_one();
    for response in [first.await.unwrap(), second.await.unwrap()] {
        assert!(response.ok, "{:?}", response.error);
        let CommandResult::Run(run) = response.result.unwrap() else {
            panic!("expected Run");
        };
        assert_eq!(run.status, "completed");
    }
    assert_eq!(agent.calls.load(Ordering::SeqCst), 2);
    assert_eq!(agent.max_active.load(Ordering::SeqCst), 1);
    assert_eq!(git(&project.workdir, &["rev-list", "--count", "HEAD"]), "3");
    assert!(git(&project.workdir, &["status", "--porcelain=v1"]).is_empty());
}

#[tokio::test]
async fn startup_recovery_interrupts_missing_and_invalid_repositories_then_continues() {
    let fixture = Fixture::new().await;
    fixture.agent.edit_workspace.store(true, Ordering::Relaxed);
    fixture
        .agent
        .stop_after_checkpoint
        .store(true, Ordering::Relaxed);
    let mut projects = vec![fixture.project.clone()];
    for number in 2..=3 {
        let path = fixture.temporary.path().join(format!("project-{number}"));
        std::fs::create_dir(&path).unwrap();
        let CommandResult::Project(project) = command(
            &fixture.service,
            Command::RegisterProject {
                id: format!("project-{number}"),
                name: format!("Project {number}"),
                workdir: path.display().to_string(),
                repo_url: None,
            },
        )
        .await
        else {
            panic!("expected Project");
        };
        command(
            &fixture.service,
            Command::CreateSession {
                id: format!("session-{number}"),
                project_id: project.id.clone(),
                agent_id: "agent".into(),
                at_message_id: None,
            },
        )
        .await;
        projects.push(project);
    }
    for session_id in ["session", "session-2", "session-3"] {
        let response = fixture
            .service
            .execute(Command::SendMessage {
                session_id: session_id.into(),
                text: format!("recover {session_id}"),
            })
            .await;
        assert_eq!(response.error.unwrap().code, ErrorCode::RunRecoveryFailed);
    }
    std::fs::remove_dir_all(&projects[0].workdir).unwrap();
    std::fs::remove_dir_all(std::path::Path::new(&projects[1].workdir).join(".git")).unwrap();

    let recovered = fixture
        .restarted_service()
        .recover_interrupted_runs()
        .await
        .unwrap();
    assert_eq!(recovered.len(), 3);
    assert_eq!(
        recovered
            .iter()
            .filter(|run| run.status == "interrupted")
            .count(),
        2
    );
    assert_eq!(
        recovered
            .iter()
            .filter(|run| run.status == "completed")
            .count(),
        1
    );
    let snapshot = fixture.store.load().await.unwrap();
    assert!(
        snapshot.value["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .all(|session| session["active_run_id"].is_null())
    );
    assert_eq!(fixture.agent.calls.load(Ordering::Relaxed), 3);
}

#[tokio::test]
async fn nonterminal_journals_reject_stale_and_nonexistent_adapter_commit_ids() {
    for commit_id in ["current", "ffffffffffffffffffffffffffffffffffffffff"] {
        let fixture = Fixture::new().await;
        fixture.agent.edit_workspace.store(true, Ordering::Relaxed);
        fixture
            .agent
            .stop_after_checkpoint
            .store(true, Ordering::Relaxed);
        let response = fixture.service.execute(send_message()).await;
        assert_eq!(response.error.unwrap().code, ErrorCode::RunRecoveryFailed);
        let snapshot = fixture.store.load().await.unwrap();
        let mut value = snapshot.value;
        let run_id = value["runs"][0]["id"].as_str().unwrap().to_owned();
        let injected = if commit_id == "current" {
            fixture.project.base_commit.clone()
        } else {
            commit_id.to_owned()
        };
        value["workspace_run_journals"][&run_id]["commit_id"] = serde_json::json!(injected);
        fixture
            .store
            .commit(snapshot.revision, value, Vec::new())
            .await
            .unwrap();

        let recovered = fixture
            .restarted_service()
            .recover_interrupted_runs()
            .await
            .unwrap();
        assert_eq!(recovered[0].status, "interrupted");
        assert_eq!(
            git(&fixture.project.workdir, &["rev-parse", "HEAD"]),
            fixture.project.base_commit
        );
        assert!(
            git(
                &fixture.project.workdir,
                &["diff", "--cached", "--name-only"]
            )
            .is_empty()
        );
    }
}

#[tokio::test]
async fn stale_result_sink_is_fenced_after_terminal_lease_handoff_without_git_side_effects() {
    let temporary = TempDir::new().unwrap();
    let project_dir = temporary.path().join("project");
    std::fs::create_dir(&project_dir).unwrap();
    let store = Arc::new(ConflictingStore {
        inner: SqliteControlStore::in_memory().unwrap(),
        checkpoints: Mutex::default(),
        failures: Mutex::default(),
        reject_runs: AtomicBool::new(false),
    });
    let agent = Arc::new(FencingAgent {
        checkpointed: Notify::new(),
        release: Notify::new(),
        late_error: Mutex::new(None),
    });
    let service = Arc::new(LocalControlService::with_workspace_agent(
        store.clone(),
        agent.clone(),
    ));
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
    let execution = {
        let service = service.clone();
        tokio::spawn(async move { service.execute(send_message()).await })
    };
    tokio::time::timeout(
        std::time::Duration::from_secs(1),
        agent.checkpointed.notified(),
    )
    .await
    .unwrap();

    let snapshot = store.load().await.unwrap();
    let mut value = snapshot.value;
    let run_id = value["runs"][0]["id"].as_str().unwrap().to_owned();
    let next_epoch = value["runs"][0]["lease_epoch"].as_u64().unwrap() + 1;
    value["runs"][0]["lease_epoch"] = serde_json::json!(next_epoch);
    value["runs"][0]["status"] = serde_json::json!("cancelled");
    value["runs"][0]["phase"] = serde_json::json!("terminal");
    value["workspace_run_journals"][&run_id]["lease_epoch"] = serde_json::json!(next_epoch);
    value["sessions"][0]["active_run_id"] = serde_json::Value::Null;
    store
        .commit(snapshot.revision, value, Vec::new())
        .await
        .unwrap();
    agent.release.notify_one();
    let response = execution.await.unwrap();
    assert_eq!(response.error.unwrap().code, ErrorCode::RunRecoveryFailed);
    let stale = agent.late_error.lock().unwrap().clone().unwrap();
    assert_eq!(stale.code, ErrorCode::RunRecoveryFailed);
    assert_eq!(stale.message, "stale workspace execution lease");
    let snapshot = store.load().await.unwrap();
    assert_eq!(snapshot.value["runs"][0]["status"], "cancelled");
    assert_eq!(
        git(&project.workdir, &["rev-parse", "HEAD"]),
        project.base_commit
    );
    assert!(git(&project.workdir, &["diff", "--cached", "--name-only"]).is_empty());
}

fn git(workdir: &str, arguments: &[&str]) -> String {
    let output = ProcessCommand::new("git")
        .arg("-C")
        .arg(workdir)
        .args(arguments)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}
