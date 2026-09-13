//! Workspace finalization regression coverage.
#![allow(clippy::pedantic)]

mod fixtures;
mod support;

use crate::fixtures::control_fixtures::{
    config, ok, send, setup, submit_run, view, wait_for_signal,
};
use crate::fixtures::workspace_agents::BlockingAgent;
use crate::support::terminal_run_status;
use ait_application::LocalControlService;
use ait_contracts::{Command, CommandResult};
use ait_domain::{DomainError, ErrorCode};
use ait_ports::{
    ControlChange, ControlFilter, ControlRead, ControlStore, ControlStoreError, DurableEvent,
    PendingEvent, WorkspaceAgent, WorkspaceAgentInvocation, WorkspaceAgentResponse,
    WorkspaceProgressReporter, WorkspaceResultSink,
};
use ait_storage_sqlite::SqliteControlStore;
use async_trait::async_trait;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::Semaphore;

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

    async fn load_progress(
        &self,
        project_id: &str,
    ) -> Result<Vec<ait_ports::ProgressCheckpoint>, ControlStoreError> {
        self.inner.load_progress(project_id).await
    }

    async fn clear_progress(&self, run_id: &str) -> Result<(), ControlStoreError> {
        self.inner.clear_progress(run_id).await
    }
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
