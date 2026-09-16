//! Deterministic disconnects across review, registration, and commit boundaries.
use super::*;
use ait_contracts::ToolApprovalAction;
use ait_domain::{
    RunPermissionProfile, ToolApprovalState, ToolApprovalTarget, ToolExecution, ToolExecutionStatus,
};
use ait_ports::*;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::time::Duration;
use tokio::sync::Semaphore;

struct Gate {
    entered: Semaphore,
    release: Semaphore,
}
impl Gate {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            entered: Semaphore::new(0),
            release: Semaphore::new(0),
        })
    }
    async fn pause(&self) {
        self.entered.add_permits(1);
        tokio::time::timeout(Duration::from_secs(10), self.release.acquire())
            .await
            .unwrap()
            .unwrap()
            .forget();
    }
    async fn entered(&self) {
        tokio::time::timeout(Duration::from_secs(10), self.entered.acquire())
            .await
            .unwrap()
            .unwrap()
            .forget();
    }
    fn release(&self) {
        self.release.add_permits(1);
    }
}
struct Observe {
    pid: AtomicU32,
    disconnected: Semaphore,
}
impl Observe {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            pid: AtomicU32::new(0),
            disconnected: Semaphore::new(0),
        })
    }
    async fn kill_and_wait_for_disconnect(&self) {
        let pid = self.pid.load(Ordering::SeqCst);
        assert!(pid > 0);
        assert!(
            std::process::Command::new("kill")
                .args(["-KILL", &pid.to_string()])
                .status()
                .unwrap()
                .success()
        );
        tokio::time::timeout(Duration::from_secs(5), self.disconnected.acquire())
            .await
            .unwrap()
            .unwrap()
            .forget();
    }
}
impl ait_ipc::supervisor::WorkerObserver for Observe {
    fn checkpoint(
        &self,
        pid: u32,
        method: &str,
        _: ait_ipc::supervisor::CommitBoundary,
    ) -> Result<(), ait_contracts::worker::ProtocolError> {
        if method == "tool_approval" {
            let _ = self
                .pid
                .compare_exchange(0, pid, Ordering::SeqCst, Ordering::SeqCst);
        }
        Ok(())
    }
    fn disconnected(&self, pid: u32) {
        if pid == self.pid.load(Ordering::SeqCst) {
            self.disconnected.add_permits(1);
        }
    }
}

struct ReviewGate {
    gate: Arc<Gate>,
    calls: AtomicUsize,
    at: usize,
}
impl RunToolFactory for ReviewGate {
    fn create(
        &self,
        root: &std::path::Path,
        profile: RunPermissionProfile,
    ) -> Result<Arc<dyn RunTool>, DomainError> {
        ait_tools::host::HostToolFactory.create(root, profile)
    }
    fn review(
        &self,
        root: &std::path::Path,
        profile: RunPermissionProfile,
        execution: &ToolExecution,
    ) -> Result<Option<ToolApprovalTarget>, DomainError> {
        if self.calls.fetch_add(1, Ordering::SeqCst) + 1 == self.at {
            tokio::runtime::Handle::current().block_on(self.gate.pause());
        }
        ait_tools::host::HostToolFactory.review(root, profile, execution)
    }
}
struct CommitGate {
    inner: Arc<SqliteControlStore>,
    gate: Arc<Gate>,
    event: &'static str,
    after: bool,
    fired: AtomicBool,
}
#[async_trait]
impl ControlStore for CommitGate {
    async fn read(&self, filters: &[ControlFilter]) -> Result<ControlRead, ControlStoreError> {
        self.inner.read(filters).await
    }
    async fn apply(
        &self,
        revision: u64,
        changes: Vec<ControlChange>,
        events: Vec<PendingEvent>,
    ) -> Result<u64, ControlStoreError> {
        let pause = events.iter().any(|event| event.kind == self.event)
            && !self.fired.swap(true, Ordering::SeqCst);
        if pause && !self.after {
            self.gate.pause().await;
        }
        let result = self.inner.apply(revision, changes, events).await;
        if pause && self.after {
            self.gate.pause().await;
        }
        result
    }
    async fn replay(
        &self,
        cursor: u64,
        limit: usize,
    ) -> Result<Vec<DurableEvent>, ControlStoreError> {
        self.inner.replay(cursor, limit).await
    }
    async fn event_bounds(&self) -> Result<EventBounds, ControlStoreError> {
        self.inner.event_bounds().await
    }
    async fn replay_page(
        &self,
        cursor: u64,
        limit: usize,
    ) -> Result<DurableEventPage, ControlStoreError> {
        self.inner.replay_page(cursor, limit).await
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

#[derive(Clone, Copy, Debug)]
enum Boundary {
    InitialReview,
    PendingBeforeCommit,
    PendingAfterCommit,
    DecisionReview,
    DecisionCommit,
    DecisionAfterCommit,
}

#[tokio::test]
async fn tool_approval_disconnect_before_registration_and_across_commit_never_authorizes() {
    for kind in [ProviderKind::OpenAI, ProviderKind::DeepSeek] {
        for boundary in [
            Boundary::InitialReview,
            Boundary::PendingBeforeCommit,
            Boundary::PendingAfterCommit,
            Boundary::DecisionReview,
            Boundary::DecisionCommit,
            Boundary::DecisionAfterCommit,
        ] {
            let mut f = Fixture::new(
                kind,
                vec![
                    response(
                        kind,
                        &[(
                            "one",
                            "write",
                            json!({"file_path":"effect.txt","content":"must not run"}),
                        )],
                    ),
                    response(kind, &[]),
                ],
                "read_only",
            )
            .await;
            let gate = Gate::new();
            let observe = Observe::new();
            let store: Arc<dyn ControlStore> =
                if matches!(boundary, Boundary::InitialReview | Boundary::DecisionReview) {
                    f.store.clone()
                } else {
                    Arc::new(CommitGate {
                        inner: f.store.clone(),
                        gate: gate.clone(),
                        event: if matches!(
                            boundary,
                            Boundary::DecisionCommit | Boundary::DecisionAfterCommit
                        ) {
                            "run.tool_approval_resolved"
                        } else {
                            "run.tool_approval_requested"
                        },
                        after: matches!(
                            boundary,
                            Boundary::PendingAfterCommit | Boundary::DecisionAfterCommit
                        ),
                        fired: AtomicBool::new(false),
                    })
                };
            f.service = LocalControlService::new(
                Arc::new(ait_project_local::LocalProjectWorkspace::default()),
                store,
            )
            .with_provider_gateway(Arc::new(Gateway))
            .with_api_tools(Arc::new(ReviewGate {
                gate: gate.clone(),
                calls: AtomicUsize::new(0),
                at: match boundary {
                    Boundary::InitialReview => 1,
                    Boundary::DecisionReview => 2,
                    _ => usize::MAX,
                },
            }))
            .with_run_dispatcher(Arc::new(
                ait_ipc::supervisor::WorkerSupervisor::new(env!("CARGO_BIN_EXE_ait-worker").into())
                    .with_observer(observe.clone()),
            ));
            let service = f.service.clone();
            let mut task = tokio::spawn(async move {
                service
                    .execute(Command::SendMessage {
                        session_id: "session".into(),
                        text: "Review a synthetic operation".into(),
                    })
                    .await
            });
            let mut decision = None;
            if matches!(
                boundary,
                Boundary::DecisionReview | Boundary::DecisionCommit | Boundary::DecisionAfterCommit
            ) {
                let pending = tokio::time::timeout(Duration::from_secs(10), async {
                    loop {
                        let runs = support::workspace(&f.service).await.runs;
                        if let Some(run) =
                            runs.into_iter().find(|run| !run.tool_approvals.is_empty())
                        {
                            break run;
                        }
                        tokio::time::sleep(Duration::from_millis(5)).await;
                    }
                })
                .await
                .unwrap();
                let service = f.service.clone();
                decision = Some(tokio::spawn(async move {
                    service
                        .resolve_tool_approval(
                            &pending.id,
                            &pending.tool_approvals[0].grant.request_id,
                            ToolApprovalAction::Approve,
                        )
                        .await
                }));
            }
            gate.entered().await;
            observe.kill_and_wait_for_disconnect().await;
            let mut accepted = false;
            if matches!(boundary, Boundary::PendingAfterCommit) {
                let run = support::workspace(&f.service).await.runs.remove(0);
                accepted = f
                    .service
                    .resolve_tool_approval(
                        &run.id,
                        &run.tool_approvals[0].grant.request_id,
                        ToolApprovalAction::Approve,
                    )
                    .await
                    .is_ok();
            }
            gate.release();
            if let Some(decision) = decision {
                accepted |= decision.await.unwrap().is_ok();
            }
            let settlement = tokio::time::timeout(Duration::from_secs(5), &mut task).await;
            let timely = settlement.is_ok();
            if !timely {
                let run = support::workspace(&f.service).await.runs.remove(0);
                let _ = f
                    .service
                    .execute(Command::CancelRun { run_id: run.id })
                    .await;
                let _ = tokio::time::timeout(Duration::from_secs(5), &mut task).await;
            }
            let workspace = support::workspace(&f.service).await;
            let run = support::persisted_run(f.store.as_ref(), &workspace.runs[0].id).await;
            let events = f.service.replay_events(0, 1000).await.unwrap();
            let old_lease_pending = events
                .iter()
                .filter(|event| event.kind == "run.tool_approval_requested")
                .count();
            assert!(
                !accepted,
                "{kind:?}/{boundary:?}: a dead worker's approval was accepted"
            );
            assert!(
                timely,
                "{kind:?}/{boundary:?}: recovery waited on a dead or resurrected waiter"
            );
            assert_eq!(run.status, "completed", "{kind:?}/{boundary:?}");
            assert!(!f.workdir.join("effect.txt").exists());
            assert!(
                run.tool_approvals
                    .iter()
                    .all(|approval| approval.status == ToolApprovalState::Expired)
            );
            assert!(
                run.tool_approvals
                    .iter()
                    .all(|approval| approval.grant.lease_epoch < run.lease_epoch)
            );
            assert_eq!(run.execution.as_ref().unwrap().tools.len(), 1);
            let tool = &run.execution.as_ref().unwrap().tools[0];
            assert_eq!(tool.status, ToolExecutionStatus::Denied);
            assert!(tool.tool_result_message_id.is_some());
            assert_eq!(
                workspace
                    .messages
                    .iter()
                    .filter(|message| message
                        .data
                        .as_ref()
                        .and_then(|data| data.get("native_message"))
                        .is_some_and(|message| message["kind"] == "tool_result"))
                    .count(),
                1
            );
            assert_eq!(f.requests.lock().unwrap().len(), 2);
            assert_eq!(
                old_lease_pending,
                usize::from(!matches!(boundary, Boundary::InitialReview))
            );
            for approval in &run.tool_approvals {
                assert!(
                    f.service
                        .resolve_tool_approval(
                            &run.id,
                            &approval.grant.request_id,
                            ToolApprovalAction::Approve
                        )
                        .await
                        .is_err()
                );
            }
            println!("PASS {kind:?}/{boundary:?}: disconnect rejected; recovered without replay");
            f.finish().await;
        }
    }
}
