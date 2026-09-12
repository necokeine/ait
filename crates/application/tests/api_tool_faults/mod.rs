//! Crash images are taken at the public store commit boundary, then reopened in SQLite.
use super::*;
use ait_ports::{
    ControlChange, ControlFilter, ControlRead, ControlStore, ControlStoreError, DurableEvent,
    DurableEventPage, EventBounds, PendingEvent, ProgressCheckpoint,
};
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use support::ControlStoreTestExt;

pub(super) struct InjectedStore {
    inner: SqliteControlStore,
    capture_cancel: AtomicBool,
    image: Mutex<Option<Value>>,
    fault_after_effect: AtomicU8,
    panic_on_attempt: AtomicBool,
}
impl InjectedStore {
    pub(super) fn new(inner: SqliteControlStore) -> Self {
        Self {
            inner,
            capture_cancel: AtomicBool::new(false),
            image: Mutex::new(None),
            fault_after_effect: AtomicU8::new(0),
            panic_on_attempt: AtomicBool::new(false),
        }
    }
}
#[async_trait]
impl ControlStore for InjectedStore {
    async fn read(&self, filters: &[ControlFilter]) -> Result<ControlRead, ControlStoreError> {
        self.inner.read(filters).await
    }
    async fn apply(
        &self,
        revision: u64,
        changes: Vec<ControlChange>,
        events: Vec<PendingEvent>,
    ) -> Result<u64, ControlStoreError> {
        let completed_effect = changes.iter().any(|change| match change {
            ControlChange::Put(record) => record.value["execution"]["tools"]
                .as_array()
                .is_some_and(|tools| tools.iter().any(|tool| tool["status"] == "succeeded")),
            _ => false,
        });
        let mut lose_ack = false;
        if completed_effect {
            let fault = self.fault_after_effect.swap(0, Ordering::SeqCst);
            if fault != 0 {
                *self.image.lock().unwrap() = Some(self.inner.load_state().await?.value);
            }
            match fault {
                1 => {
                    return Err(ControlStoreError::Other(
                        "injected tool outcome commit failure".into(),
                    ));
                }
                2 => panic!("injected task panic before tool outcome commit"),
                3 => lose_ack = true,
                _ => {}
            }
        }
        let cancelled_tool = changes.iter().any(|change| match change {
            ControlChange::Put(record) => record.value["execution"]["tools"]
                .as_array()
                .is_some_and(|tools| {
                    tools.iter().any(|tool| {
                        tool["status"] == "cancelled" && tool["tool_result_message_id"].is_null()
                    })
                }),
            _ => false,
        });
        let calling_agent = changes.iter().any(|change| match change {
            ControlChange::Put(record) => {
                record.value["execution"]["run"]["phase"] == "calling_agent"
            }
            _ => false,
        });
        let revision = self.inner.apply(revision, changes, events).await?;
        if calling_agent && self.panic_on_attempt.swap(false, Ordering::SeqCst) {
            panic!("injected task panic with a Running attempt");
        }
        if lose_ack {
            return Err(ControlStoreError::Other(
                "injected lost outcome acknowledgment".into(),
            ));
        }
        if cancelled_tool && self.capture_cancel.swap(false, Ordering::SeqCst) {
            *self.image.lock().unwrap() = Some(self.inner.load_state().await?.value);
        }
        Ok(revision)
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
        p: ProgressCheckpoint,
        events: Vec<PendingEvent>,
    ) -> Result<(), ControlStoreError> {
        self.inner.save_progress(p, events).await
    }
    async fn load_progress(
        &self,
        project: &str,
    ) -> Result<Vec<ProgressCheckpoint>, ControlStoreError> {
        self.inner.load_progress(project).await
    }
    async fn clear_progress(&self, run: &str) -> Result<(), ControlStoreError> {
        self.inner.clear_progress(run).await
    }
}

#[tokio::test]
async fn store_error_terminalizes_api_children_and_restart_does_not_replay() {
    failed_effect_is_settled(1).await;
}
#[tokio::test]
async fn task_panic_terminalizes_api_children_and_restart_does_not_replay() {
    failed_effect_is_settled(2).await;
}
async fn failed_effect_is_settled(fault: u8) {
    let kind = ProviderKind::OpenAI;
    let f = Fixture::new(
        kind,
        vec![response(
            kind,
            &[(
                "write",
                "write",
                json!({"file_path":"once","content":"one effect"}),
            )],
        )],
        "workspace_write",
    )
    .await;
    f.store.fault_after_effect.store(fault, Ordering::SeqCst);
    let run = f.run().await;
    assert_eq!(run.status, "failed");
    let execution = run.execution.as_ref().unwrap();
    assert_eq!(
        execution.run.status,
        ait_domain::RunStatus::Failed,
        "canonical Run must agree with public terminal state"
    );
    assert!(
        execution
            .attempts
            .iter()
            .all(|a| a.status != ait_domain::RunAttemptStatus::Running)
    );
    assert!(
        execution.tools.iter().all(|t| t.status.is_terminal()),
        "unknown effect must be terminal with an explicit diagnostic"
    );
    assert_eq!(
        execution.tools[0].error.as_ref().unwrap().code,
        ait_domain::ErrorCode::RunRecoveryFailed
    );
    assert!(execution.tools[0].tool_result_message_id.is_some());
    assert!(
        support::workspace(&f.service).await.sessions[0]
            .active_run_id
            .is_none()
    );
    assert_eq!(
        std::fs::read_to_string(f.project.path().join("once")).unwrap(),
        "one effect"
    );
    // Replace the effect externally; an unsafe replay would overwrite this marker.
    std::fs::write(f.project.path().join("once"), "external marker").unwrap();
    let restarted = LocalControlService::new(Arc::new(
        SqliteControlStore::open(f.directory.path().join("ait.db")).unwrap(),
    ));
    restarted.recover_interrupted_runs().await.unwrap();
    let CommandResult::Run(saved) = ok(
        &restarted,
        Command::GetRun {
            run_id: run.id.clone(),
        },
    )
    .await
    else {
        panic!()
    };
    assert_eq!(saved, run);
    assert_eq!(
        std::fs::read_to_string(f.project.path().join("once")).unwrap(),
        "external marker"
    );
    assert_eq!(f.requests.lock().unwrap().len(), 1);
    f.finish().await;
}
async fn wait_run(
    service: &LocalControlService,
    id: &str,
    predicate: impl Fn(&ait_contracts::RunView) -> bool,
) -> ait_contracts::RunView {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let CommandResult::Run(run) = ok(service, Command::GetRun { run_id: id.into() }).await
            else {
                panic!()
            };
            if predicate(&run) {
                return run;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap()
}

#[cfg(unix)]
#[tokio::test]
async fn durable_cancel_survives_a_crash_after_an_intermediate_tool_save() {
    let kind = ProviderKind::OpenAI;
    let f = Fixture::new(
        kind,
        vec![response(
            kind,
            &[(
                "sleep",
                "bash",
                json!({"command":"sleep 30","description":"Wait"}),
            )],
        )],
        "read_only",
    )
    .await;
    f.store.capture_cancel.store(true, Ordering::SeqCst);
    let service = Arc::new(f.service.clone());
    let accepted = service
        .submit(Command::SendMessage {
            session_id: "session".into(),
            text: "Wait".into(),
        })
        .await;
    let Some(CommandResult::Run(initial)) = accepted.result else {
        panic!()
    };
    wait_run(&service, &initial.id, |r| {
        r.execution.as_ref().is_some_and(|e| {
            e.tools
                .iter()
                .any(|t| t.status == ait_domain::ToolExecutionStatus::Running)
        })
    })
    .await;
    ok(
        &service,
        Command::CancelRun {
            run_id: initial.id.clone(),
        },
    )
    .await;
    wait_run(&service, &initial.id, |r| r.status == "cancelled").await;
    let image = f
        .store
        .image
        .lock()
        .unwrap()
        .clone()
        .expect("captured the commit before ToolResult/terminal");
    assert_eq!(
        image["runs"][0]["status"], "cancelling",
        "intermediate saves must preserve the durable cancel intent"
    );
    let restore_path = f.directory.path().join("crash-image.db");
    let restored = SqliteControlStore::open(&restore_path).unwrap();
    let revision = restored.load_state().await.unwrap().revision;
    restored
        .replace_state(revision, image, vec![])
        .await
        .unwrap();
    drop(restored);
    // No gateway or tool factory is installed: recovery must consume cancellation
    // without even preparing an executor or requesting credentials.
    let restarted =
        LocalControlService::new(Arc::new(SqliteControlStore::open(&restore_path).unwrap()));
    restarted.recover_interrupted_runs().await.unwrap();
    let state = support::workspace(&restarted).await;
    let execution = state.runs[0].execution.as_ref().unwrap();
    assert_eq!(state.runs[0].status, "cancelled");
    assert_eq!(execution.run.status, ait_domain::RunStatus::Cancelled);
    assert_eq!(execution.tools.len(), 1);
    assert_eq!(
        execution.tools[0].status,
        ait_domain::ToolExecutionStatus::Cancelled
    );
    assert!(execution.tools[0].tool_result_message_id.is_some());
    assert_eq!(
        state
            .messages
            .iter()
            .filter(|m| m.kind == "tool_result")
            .count(),
        1
    );
    assert!(state.sessions[0].active_run_id.is_none());
    assert_eq!(f.requests.lock().unwrap().len(), 1);
    f.finish().await;
}

#[tokio::test]
async fn startup_repairs_old_terminal_projection_without_stealing_a_moved_session() {
    let kind = ProviderKind::OpenAI;
    let f = Fixture::new(
        kind,
        vec![response(
            kind,
            &[(
                "write",
                "write",
                json!({"file_path":"once","content":"one effect"}),
            )],
        )],
        "workspace_write",
    )
    .await;
    f.store.fault_after_effect.store(1, Ordering::SeqCst);
    let finished = f.run().await;
    for moved in [false, true] {
        let mut image = f.store.image.lock().unwrap().clone().unwrap();
        // Exactly the old fallback shape: terminal projection, Running canonical
        // records, Session released, and no result Message for the unknown effect.
        image["runs"][0]["status"] = json!("failed");
        image["runs"][0]["phase"] = json!("terminal");
        image["sessions"][0]["active_run_id"] = Value::Null;
        if moved {
            image["sessions"][0]["current_message_id"] =
                image["runs"][0]["base_message_id"].clone();
        }
        let old_head = image["sessions"][0]["current_message_id"]
            .as_str()
            .unwrap()
            .to_owned();
        let path = f.directory.path().join(format!("old-{moved}.db"));
        let restore = SqliteControlStore::open(&path).unwrap();
        let revision = restore.load_state().await.unwrap().revision;
        restore
            .replace_state(revision, image, vec![])
            .await
            .unwrap();
        drop(restore);
        let restarted =
            LocalControlService::new(Arc::new(SqliteControlStore::open(&path).unwrap()));
        restarted.recover_interrupted_runs().await.unwrap();
        let state = support::workspace(&restarted).await;
        let execution = state.runs[0].execution.as_ref().unwrap();
        assert_eq!(state.runs[0].id, finished.id);
        assert_eq!(execution.run.status, ait_domain::RunStatus::Failed);
        assert!(execution.tools.iter().all(|t| t.status.is_terminal()));
        assert!(execution.tools[0].tool_result_message_id.is_some());
        assert!(state.sessions[0].active_run_id.is_none());
        if moved {
            assert_eq!(state.sessions[0].current_message_id, old_head);
        } else {
            assert_eq!(
                Some(&state.sessions[0].current_message_id),
                state.runs[0].last_message_id.as_ref()
            );
        }
        assert_eq!(f.requests.lock().unwrap().len(), 1);
    }
    f.finish().await;
}

#[tokio::test]
async fn panic_with_running_attempt_settles_before_session_release() {
    let f = Fixture::new(ProviderKind::OpenAI, vec![], "read_only").await;
    f.store.panic_on_attempt.store(true, Ordering::SeqCst);
    let run = f.run().await;
    let execution = run.execution.as_ref().unwrap();
    assert_eq!(run.status, "failed");
    assert_eq!(execution.run.status, ait_domain::RunStatus::Failed);
    assert_eq!(execution.attempts.len(), 1);
    assert_eq!(
        execution.attempts[0].status,
        ait_domain::RunAttemptStatus::Failed
    );
    assert!(execution.attempts[0].ended_at.is_some());
    assert!(
        support::workspace(&f.service).await.sessions[0]
            .active_run_id
            .is_none()
    );
    let restarted = LocalControlService::new(Arc::new(
        SqliteControlStore::open(f.directory.path().join("ait.db")).unwrap(),
    ));
    restarted.recover_interrupted_runs().await.unwrap();
    assert_eq!(support::workspace(&restarted).await.runs[0], run);
    assert!(f.requests.lock().unwrap().is_empty());
    f.finish().await;
}

#[tokio::test]
async fn lost_outcome_ack_keeps_stable_tool_result_order() {
    let kind = ProviderKind::OpenAI;
    let f = Fixture::new(
        kind,
        vec![response(
            kind,
            &[
                ("first", "grep", json!({"pattern":"one"})),
                ("second", "grep", json!({"pattern":"two"})),
            ],
        )],
        "read_only",
    )
    .await;
    f.store.fault_after_effect.store(3, Ordering::SeqCst);
    let run = f.run().await;
    assert_eq!(run.status, "failed");
    let state = support::workspace(&f.service).await;
    let mut results = state
        .messages
        .iter()
        .filter(|m| m.kind == "tool_result")
        .collect::<Vec<_>>();
    results.sort_by_key(|m| {
        m.data.as_ref().unwrap()["native_message"]["run_seq"]
            .as_u64()
            .unwrap()
    });
    let calls = results
        .iter()
        .map(|m| {
            m.data.as_ref().unwrap()["native_message"]["tool_result"]["call_id"]
                .as_str()
                .unwrap()
        })
        .collect::<Vec<_>>();
    assert_eq!(
        calls,
        ["first", "second"],
        "terminal repair must sort by ToolUse position, not last-updated storage order"
    );
    let restarted = LocalControlService::new(Arc::new(
        SqliteControlStore::open(f.directory.path().join("ait.db")).unwrap(),
    ));
    restarted.recover_interrupted_runs().await.unwrap();
    assert_eq!(support::workspace(&restarted).await.runs[0], run);
    assert_eq!(f.requests.lock().unwrap().len(), 1);
    f.finish().await;
}

#[derive(Default)]
struct PublishPause {
    entered: tokio::sync::Notify,
    released: Mutex<bool>,
    wake: std::sync::Condvar,
}
impl ait_tools::host::HostIoObserver for PublishPause {
    fn checkpoint(&self, _: &str, point: ait_tools::host::HostIoCheckpoint) {
        if point == ait_tools::host::HostIoCheckpoint::BeforePublish {
            self.entered.notify_one();
            let _guard = self
                .wake
                .wait_while(self.released.lock().unwrap(), |released| !*released)
                .unwrap();
        }
    }
}
#[tokio::test]
async fn cancelling_a_blocked_host_write_keeps_session_until_worker_cleanup() {
    let kind = ProviderKind::OpenAI;
    let f = Fixture::new(
        kind,
        vec![response(
            kind,
            &[(
                "write",
                "write",
                json!({"file_path":"late","content":"forbidden late effect"}),
            )],
        )],
        "workspace_write",
    )
    .await;
    let pause = Arc::new(PublishPause::default());
    let service = Arc::new(f.service.clone().with_api_tools(Arc::new(
        ait_tools::host::HostToolFactory::with_observer(pause.clone()),
    )));
    let accepted = service
        .submit(Command::SendMessage {
            session_id: "session".into(),
            text: "Write".into(),
        })
        .await;
    let Some(CommandResult::Run(initial)) = accepted.result else {
        panic!()
    };
    pause.entered.notified().await;
    ok(
        &service,
        Command::CancelRun {
            run_id: initial.id.clone(),
        },
    )
    .await;
    tokio::time::sleep(std::time::Duration::from_millis(30)).await;
    let during = support::workspace(&service).await;
    *pause.released.lock().unwrap() = true;
    pause.wake.notify_all();
    let final_run = wait_run(&service, &initial.id, |r| r.status == "cancelled").await;
    assert_eq!(during.runs[0].status, "cancelling");
    assert_eq!(
        during.sessions[0].active_run_id.as_deref(),
        Some(initial.id.as_str())
    );
    assert!(!f.project.path().join("late").exists());
    assert!(std::fs::read_dir(f.project.path()).unwrap().all(|p| {
        !p.unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".ait-tool-")
    }));
    let execution = final_run.execution.unwrap();
    assert_eq!(
        execution.tools[0].status,
        ait_domain::ToolExecutionStatus::Cancelled
    );
    assert!(execution.tools[0].tool_result_message_id.is_some());
    assert!(
        support::workspace(&service).await.sessions[0]
            .active_run_id
            .is_none()
    );
    assert_eq!(f.requests.lock().unwrap().len(), 1);
    f.finish().await;
}
