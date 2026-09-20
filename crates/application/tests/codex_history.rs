//! Native Codex history synchronization and writable continuation coverage.
#![allow(clippy::pedantic)]

use std::sync::{Arc, Mutex};

use ait_application::LocalControlService;
use ait_contracts::{AgentConfiguration, Command, CommandResult};
use ait_domain::{DomainError, SessionSource};
use ait_ports::{
    CodexHistorySource, CodexItemsView, CodexPreparedThread, CodexThreadConnection,
    CodexThreadInvocation, CodexThreadSnapshot, CodexThreadSourceKind, CodexThreadWriter,
    CodexTurnSnapshot, WorkspaceProgressReporter,
};
use ait_storage_sqlite::SqliteControlStore;
use async_trait::async_trait;
use serde_json::json;

#[path = "codex_history/project_listing.rs"]
mod project_listing;

#[derive(Debug)]
struct NativeCodexFixture {
    snapshot: Arc<Mutex<CodexThreadSnapshot>>,
    writes: Arc<Mutex<Vec<CodexThreadInvocation>>>,
}

#[async_trait]
impl CodexHistorySource for NativeCodexFixture {
    async fn list_threads(
        &self,
        _source_kinds: &[CodexThreadSourceKind],
    ) -> Result<Vec<CodexThreadSnapshot>, DomainError> {
        let mut summary = self.snapshot.lock().unwrap().clone();
        summary.turns.clear();
        Ok(vec![summary])
    }

    async fn read_thread(&self, thread_id: &str) -> Result<CodexThreadSnapshot, DomainError> {
        let snapshot = self.snapshot.lock().unwrap();
        assert_eq!(thread_id, snapshot.id);
        Ok(snapshot.clone())
    }
}

struct FixtureConnection {
    request: CodexThreadInvocation,
    resumed: CodexPreparedThread,
    snapshot: Arc<Mutex<CodexThreadSnapshot>>,
    writes: Arc<Mutex<Vec<CodexThreadInvocation>>>,
}

#[async_trait]
impl CodexThreadWriter for NativeCodexFixture {
    async fn open(
        &self,
        request: CodexThreadInvocation,
    ) -> Result<Box<dyn CodexThreadConnection>, DomainError> {
        let mut history = self.snapshot.lock().unwrap().clone();
        if history.status["type"] == "active" {
            return Err(DomainError {
                code: ait_domain::ErrorCode::CodexThreadWriterBusy,
                message: "thread already has an active writer".into(),
                retryable: false,
                cause_id: None,
                details: None,
            });
        }
        history.writer_confirmed = true;
        history.status = json!({"type":"idle"});
        Ok(Box::new(FixtureConnection {
            resumed: CodexPreparedThread {
                history,
                model: request.model.clone(),
                model_provider: "openai".into(),
                reasoning_effort: request.reasoning_effort.clone(),
            },
            request,
            snapshot: self.snapshot.clone(),
            writes: self.writes.clone(),
        }))
    }
}

#[async_trait]
impl CodexThreadConnection for FixtureConnection {
    fn prepared(&self) -> &CodexPreparedThread {
        &self.resumed
    }
    async fn start(
        &mut self,
        _progress: Arc<dyn WorkspaceProgressReporter>,
    ) -> Result<CodexThreadSnapshot, DomainError> {
        self.writes.lock().unwrap().push(self.request.clone());
        let mut snapshot = self.snapshot.lock().unwrap();
        if snapshot.metadata.get("reject") == Some(&json!(true)) {
            return Err(DomainError::invariant(
                ait_domain::ErrorCode::CodexInputNotAccepted,
                "turn/start rejected input",
            ));
        }
        let status = snapshot
            .metadata
            .get("turn_status")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("completed")
            .to_owned();
        let id = format!("turn-{}", snapshot.turns.len() + 1);
        snapshot.turns.push(CodexTurnSnapshot {
            id: id.clone(), status, items_view: CodexItemsView::Full,
            started_at: Some(21), completed_at: Some(30), error: None,
            items: vec![json!({"id":format!("{id}-user"),"type":"userMessage","clientId":self.request.request_id,"content":[{"type":"text","text":self.request.prompt}]}),
                json!({"id":format!("{id}-agent"),"type":"agentMessage","text":"continued native Thread"})],
        });
        snapshot.writer_confirmed = true;
        snapshot.status = json!({"type":"idle"});
        if snapshot.metadata.get("limited") == Some(&json!(true)) {
            let mut failure = DomainError::invariant(
                ait_domain::ErrorCode::RunLimitExceeded,
                "Codex streamed text output (bytes) limit exceeded: observed 8388609, limit 8388608",
            );
            failure.details = Some(
                serde_json::from_value(json!({
                    "metric":"text_bytes", "actual":8_388_609, "limit":8_388_608,
                }))
                .unwrap(),
            );
            return Err(failure);
        }
        if snapshot.metadata.get("unknown") == Some(&json!(true)) {
            return Err(DomainError::invariant(
                ait_domain::ErrorCode::CodexInputOutcomeUnknown,
                "connection lost after send",
            ));
        }
        Ok(snapshot.clone())
    }
    async fn read(&mut self) -> Result<CodexThreadSnapshot, DomainError> {
        let mut snapshot = self.snapshot.lock().unwrap().clone();
        snapshot.writer_confirmed = true;
        Ok(snapshot)
    }
    async fn close(&mut self) {}
}

fn snapshot(cwd: String) -> CodexThreadSnapshot {
    CodexThreadSnapshot {
        writer_confirmed: false,
        id: "native-thread".into(),
        session_id: "native-session".into(),
        forked_from_id: None,
        cwd,
        project_id: Some("provider-project".into()),
        name: Some("Native history".into()),
        preview: "hello".into(),
        source: json!("vscode"),
        history_mode: "paginated".into(),
        status: json!({"type": "notLoaded"}),
        archived: false,
        created_at: 10,
        updated_at: 20,
        turns: vec![CodexTurnSnapshot {
            id: "turn-1".into(),
            status: "completed".into(),
            items: vec![
                json!({
                    "id": "user-1",
                    "type": "userMessage",
                    "content": [{"type": "text", "text": "hello"}],
                }),
                json!({
                    "id": "agent-1",
                    "type": "agentMessage",
                    "text": "hi",
                }),
            ],
            items_view: CodexItemsView::Full,
            error: None,
            started_at: Some(10),
            completed_at: Some(20),
        }],
        metadata: serde_json::Map::new(),
    }
}

async fn ok(service: &LocalControlService, command: Command) -> CommandResult {
    let response = service.execute(command).await;
    assert!(response.ok, "{:?}", response.error);
    response.result.unwrap()
}

async fn register_binding(service: &LocalControlService, cwd: &std::path::Path) {
    ok(
        service,
        Command::RegisterProject {
            id: "project".into(),
            name: "Project".into(),
            workdir: Some(cwd.display().to_string()),
            repo_url: None,
        },
    )
    .await;
    ok(
        service,
        Command::RegisterAgent {
            id: "agent".into(),
            name: "Codex".into(),
            config: AgentConfiguration {
                provider_id: "builtin-codex".into(),
                model: "gpt-5.6-sol".into(),
                reasoning_effort: Some("medium".into()),
                system_prompt: None,
            },
        },
    )
    .await;
}

#[tokio::test]
async fn imported_thread_is_idempotent_and_continues_in_native_cwd() {
    let directory = tempfile::tempdir().unwrap();
    let cwd = directory.path().canonicalize().unwrap();
    let fixture = Arc::new(NativeCodexFixture {
        snapshot: Arc::new(Mutex::new(snapshot(cwd.display().to_string()))),
        writes: Arc::default(),
    });
    let store = Arc::new(SqliteControlStore::in_memory().unwrap());
    let service = LocalControlService::new(
        Arc::new(ait_workspace_local::LocalProjectWorkspace::default()),
        store,
    )
    .with_codex_history_source(fixture.clone())
    .with_codex_thread_writer(fixture.clone());

    register_binding(&service, &cwd).await;

    let CommandResult::CodexThreads(threads) = ok(
        &service,
        Command::ListCodexThreads {
            provider_id: "builtin-codex".into(),
            project_id: None,
        },
    )
    .await
    else {
        panic!("expected Codex Threads")
    };
    assert_eq!(threads.len(), 1);
    assert_eq!(threads[0].thread_id, "native-thread");

    let sync = || Command::SyncCodexThread {
        provider_id: "builtin-codex".into(),
        thread_id: "native-thread".into(),
        project_id: "project".into(),
        agent_id: "agent".into(),
    };
    let CommandResult::Session(first) = ok(&service, sync()).await else {
        panic!("expected imported Session")
    };
    let CommandResult::Session(second) = ok(&service, sync()).await else {
        panic!("expected reconciled Session")
    };
    assert_eq!(first.id, second.id);
    assert_eq!(first.current_message_id, second.current_message_id);
    assert!(matches!(first.source, SessionSource::CodexThread(_)));

    std::fs::write(cwd.join("dirty.txt"), "native clients may leave changes").unwrap();
    let CommandResult::Run(run) = ok(
        &service,
        Command::SendMessage {
            session_id: first.id.clone(),
            text: "continue here".into(),
        },
    )
    .await
    else {
        panic!("expected completed Run")
    };
    assert_eq!(run.status, "completed");
    {
        let writes = fixture.writes.lock().unwrap();
        assert_eq!(writes.len(), 1);
        assert_eq!(writes[0].thread_id.as_deref(), Some("native-thread"));
        assert_eq!(writes[0].cwd, cwd);
        assert_eq!(writes[0].prompt, "continue here");
    }

    let CommandResult::Messages(messages) = ok(
        &service,
        Command::ListMessages {
            project_id: "project".into(),
        },
    )
    .await
    else {
        panic!("expected Messages")
    };
    let submitted = messages
        .iter()
        .find(|message| message.text.as_deref() == Some("continue here"))
        .unwrap();
    assert!(submitted.git_commit.is_none());
    assert_eq!(run.base_message_id, first.current_message_id);
    assert_eq!(
        submitted
            .data
            .as_ref()
            .unwrap()
            .pointer("/native_message/run_id"),
        Some(&json!(run.id))
    );
    assert_eq!(
        submitted
            .data
            .as_ref()
            .unwrap()
            .pointer("/native_message/run_seq"),
        Some(&json!(1))
    );
    let before = serde_json::to_value(&messages).unwrap();
    let CommandResult::Session(synced) = ok(&service, sync()).await else {
        panic!("Session");
    };
    let CommandResult::Messages(after) = ok(
        &service,
        Command::ListMessages {
            project_id: "project".into(),
        },
    )
    .await
    else {
        panic!("Messages");
    };
    assert_eq!(before, serde_json::to_value(&after).unwrap());
    assert_eq!(Some(synced.current_message_id), run.last_message_id);
}

#[tokio::test]
async fn imported_thread_rejects_input_while_provider_reports_external_activity() {
    let directory = tempfile::tempdir().unwrap();
    let cwd = directory.path().canonicalize().unwrap();
    let mut active = snapshot(cwd.display().to_string());
    active.status = json!({"type": "active"});
    let fixture = Arc::new(NativeCodexFixture {
        snapshot: Arc::new(Mutex::new(active)),
        writes: Arc::default(),
    });
    let service = LocalControlService::new(
        Arc::new(ait_workspace_local::LocalProjectWorkspace::default()),
        Arc::new(SqliteControlStore::in_memory().unwrap()),
    )
    .with_codex_history_source(fixture.clone())
    .with_codex_thread_writer(fixture.clone());
    register_binding(&service, &cwd).await;
    let CommandResult::Session(session) = ok(
        &service,
        Command::SyncCodexThread {
            provider_id: "builtin-codex".into(),
            thread_id: "native-thread".into(),
            project_id: "project".into(),
            agent_id: "agent".into(),
        },
    )
    .await
    else {
        panic!("expected imported Session")
    };

    let before = ok(
        &service,
        Command::ListMessages {
            project_id: "project".into(),
        },
    )
    .await;
    let response = service
        .execute(Command::SendMessage {
            session_id: session.id,
            text: "must not be sent".into(),
        })
        .await;
    assert_eq!(
        response.error.unwrap().code,
        ait_domain::ErrorCode::CodexThreadWriterBusy
    );
    let after = ok(
        &service,
        Command::ListMessages {
            project_id: "project".into(),
        },
    )
    .await;
    assert_eq!(
        serde_json::to_value(before).unwrap(),
        serde_json::to_value(after).unwrap()
    );
    assert!(fixture.writes.lock().unwrap().is_empty());
}

fn sync() -> Command {
    Command::SyncCodexThread {
        provider_id: "builtin-codex".into(),
        thread_id: "native-thread".into(),
        project_id: "project".into(),
        agent_id: "agent".into(),
    }
}

async fn setup() -> (
    tempfile::TempDir,
    Arc<NativeCodexFixture>,
    LocalControlService,
    ait_contracts::SessionView,
) {
    let directory = tempfile::tempdir().unwrap();
    let cwd = directory.path().canonicalize().unwrap();
    let fixture = Arc::new(NativeCodexFixture {
        snapshot: Arc::new(Mutex::new(snapshot(cwd.display().to_string()))),
        writes: Arc::default(),
    });
    let service = LocalControlService::new(
        Arc::new(ait_workspace_local::LocalProjectWorkspace::default()),
        Arc::new(SqliteControlStore::in_memory().unwrap()),
    )
    .with_codex_history_source(fixture.clone())
    .with_codex_thread_writer(fixture.clone());
    register_binding(&service, &cwd).await;
    let CommandResult::Session(session) = ok(&service, sync()).await else {
        panic!("Session");
    };
    (directory, fixture, service, session)
}

async fn messages(service: &LocalControlService) -> Vec<ait_contracts::MessageView> {
    let CommandResult::Messages(messages) = ok(
        service,
        Command::ListMessages {
            project_id: "project".into(),
        },
    )
    .await
    else {
        panic!("Messages");
    };
    messages
}

#[tokio::test]
async fn rejected_turn_start_does_not_publish_optimistic_input() {
    let (_directory, fixture, service, session) = setup().await;
    fixture
        .snapshot
        .lock()
        .unwrap()
        .metadata
        .insert("reject".into(), json!(true));
    let before = messages(&service).await;
    let CommandResult::Run(run) = ok(
        &service,
        Command::SendMessage {
            session_id: session.id,
            text: "rejected".into(),
        },
    )
    .await
    else {
        panic!("Run");
    };
    assert_eq!(
        run.error.unwrap().code,
        ait_domain::ErrorCode::CodexInputNotAccepted
    );
    assert_eq!(
        serde_json::to_value(before).unwrap(),
        serde_json::to_value(messages(&service).await).unwrap()
    );
}

#[tokio::test]
async fn interrupted_cold_tail_is_confirmed_under_writer_and_remains_stable_on_sync() {
    let (_directory, fixture, service, session) = setup().await;
    {
        let mut history = fixture.snapshot.lock().unwrap();
        history.turns[0].status = "interrupted".into();
        history.turns[0].completed_at = None;
    }
    ok(&service, sync()).await;
    let CommandResult::Run(run) = ok(
        &service,
        Command::SendMessage {
            session_id: session.id,
            text: "resume cold tail".into(),
        },
    )
    .await
    else {
        panic!("Run");
    };
    assert_eq!(run.status, "completed");
    {
        let mut history = fixture.snapshot.lock().unwrap();
        history.writer_confirmed = false;
        history.status = json!({"type":"notLoaded"});
    }
    let CommandResult::Session(synced) = ok(&service, sync()).await else {
        panic!("Session");
    };
    assert_eq!(Some(synced.current_message_id), run.last_message_id);
    assert_eq!(fixture.snapshot.lock().unwrap().turns[0].completed_at, None);
}

fn append_external(history: &mut CodexThreadSnapshot) {
    let mut turn = history.turns[0].clone();
    turn.id = "external-turn".into();
    turn.items = vec![
        json!({"id":"external-input","type":"userMessage","content":[{"type":"text","text":"external change"}]}),
    ];
    history.turns.push(turn);
    history.updated_at += 1;
}

#[tokio::test]
async fn admission_refreshes_external_head_before_freezing_run_base() {
    let (_directory, fixture, service, session) = setup().await;
    append_external(&mut fixture.snapshot.lock().unwrap());
    let CommandResult::Run(run) = ok(
        &service,
        Command::SendMessage {
            session_id: session.id,
            text: "new input".into(),
        },
    )
    .await
    else {
        panic!("Run");
    };
    assert_eq!(run.status, "completed");
    let messages = messages(&service).await;
    let base = messages
        .iter()
        .find(|message| message.id == run.base_message_id)
        .unwrap();
    assert_eq!(base.text.as_deref(), Some("external change"));
    assert_ne!(run.base_message_id, session.current_message_id);
}

#[tokio::test]
async fn unknown_send_is_correlated_by_later_sync_without_resending() {
    let (_directory, fixture, service, session) = setup().await;
    fixture
        .snapshot
        .lock()
        .unwrap()
        .metadata
        .insert("unknown".into(), json!(true));
    let before = messages(&service).await;
    let CommandResult::Run(run) = ok(
        &service,
        Command::SendMessage {
            session_id: session.id,
            text: "accepted before disconnect".into(),
        },
    )
    .await
    else {
        panic!("Run");
    };
    assert_eq!(
        run.error.unwrap().code,
        ait_domain::ErrorCode::CodexInputOutcomeUnknown
    );
    assert_eq!(messages(&service).await.len(), before.len());
    ok(&service, sync()).await;
    let CommandResult::Run(reconciled) = ok(
        &service,
        Command::GetRun {
            run_id: run.id.clone(),
        },
    )
    .await
    else {
        panic!("Run");
    };
    assert_eq!(reconciled.status, "completed");
    let events = service.replay_events(0, 1000).await.unwrap();
    assert!(events.iter().any(|event| event.kind == "run.updated"
        && event.body["id"] == run.id
        && event.body["status"] == "completed"));
    let all = messages(&service).await;
    assert_eq!(
        all.iter()
            .filter(|m| m.text.as_deref() == Some("accepted before disconnect"))
            .count(),
        1
    );
    assert_eq!(
        all.iter()
            .find(|m| Some(&m.id) == reconciled.last_message_id.as_ref())
            .unwrap()
            .data
            .as_ref()
            .unwrap()
            .pointer("/native_message/run_id"),
        Some(&json!(run.id))
    );
    assert_eq!(fixture.writes.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn failed_native_turn_publishes_accepted_input_and_preserves_failure() {
    let (_directory, fixture, service, session) = setup().await;
    fixture
        .snapshot
        .lock()
        .unwrap()
        .metadata
        .insert("turn_status".into(), json!("failed"));
    let CommandResult::Run(run) = ok(
        &service,
        Command::SendMessage {
            session_id: session.id,
            text: "accepted failure".into(),
        },
    )
    .await
    else {
        panic!("Run");
    };
    assert_eq!(run.status, "failed");
    assert!(run.last_message_id.is_some());
    assert_eq!(
        messages(&service)
            .await
            .iter()
            .filter(|m| m.text.as_deref() == Some("accepted failure"))
            .count(),
        1
    );
    let before = serde_json::to_value(messages(&service).await).unwrap();
    ok(&service, sync()).await;
    assert_eq!(
        serde_json::to_value(messages(&service).await).unwrap(),
        before
    );
}

struct DelayedHistory {
    inner: Arc<NativeCodexFixture>,
    first: std::sync::atomic::AtomicBool,
    captured: tokio::sync::Notify,
    release: tokio::sync::Notify,
}

#[async_trait]
impl CodexHistorySource for DelayedHistory {
    async fn list_threads(
        &self,
        kinds: &[CodexThreadSourceKind],
    ) -> Result<Vec<CodexThreadSnapshot>, DomainError> {
        self.inner.list_threads(kinds).await
    }
    async fn read_thread(&self, thread_id: &str) -> Result<CodexThreadSnapshot, DomainError> {
        let history = self.inner.read_thread(thread_id).await?;
        if self.first.swap(false, std::sync::atomic::Ordering::SeqCst) {
            self.captured.notify_one();
            self.release.notified().await;
        }
        Ok(history)
    }
}

#[tokio::test]
async fn delayed_older_sync_rereads_after_conflict_instead_of_rewinding_head() {
    let (_directory, fixture, service, session) = setup().await;
    let delayed = Arc::new(DelayedHistory {
        inner: fixture.clone(),
        first: true.into(),
        captured: tokio::sync::Notify::new(),
        release: tokio::sync::Notify::new(),
    });
    let old_service = service.clone().with_codex_history_source(delayed.clone());
    let old_read = tokio::spawn(async move { old_service.execute(sync()).await });
    delayed.captured.notified().await;
    append_external(&mut fixture.snapshot.lock().unwrap());
    let CommandResult::Session(newer) = ok(&service, sync()).await else {
        panic!("Session");
    };
    assert_ne!(newer.current_message_id, session.current_message_id);
    delayed.release.notify_one();
    let response = tokio::time::timeout(std::time::Duration::from_secs(5), old_read)
        .await
        .unwrap()
        .unwrap();
    assert!(response.ok, "{:?}", response.error);
    let CommandResult::Session(after) = response.result.unwrap() else {
        panic!("Session");
    };
    assert_eq!(after.current_message_id, newer.current_message_id);
}

#[tokio::test]
async fn restart_reconciles_durable_unknown_input_without_replaying_it() {
    use ait_ports::{ControlChange, ControlFilter, ControlRecordKind, ControlStore};
    let directory = tempfile::tempdir().unwrap();
    let cwd = directory.path().canonicalize().unwrap();
    let fixture = Arc::new(NativeCodexFixture {
        snapshot: Arc::new(Mutex::new(snapshot(cwd.display().to_string()))),
        writes: Arc::default(),
    });
    fixture
        .snapshot
        .lock()
        .unwrap()
        .metadata
        .insert("unknown".into(), json!(true));
    let store = Arc::new(SqliteControlStore::in_memory().unwrap());
    let service = LocalControlService::new(
        Arc::new(ait_workspace_local::LocalProjectWorkspace::default()),
        store.clone(),
    )
    .with_codex_history_source(fixture.clone())
    .with_codex_thread_writer(fixture.clone());
    register_binding(&service, &cwd).await;
    let CommandResult::Session(session) = ok(&service, sync()).await else {
        panic!("Session");
    };
    let CommandResult::Run(run) = ok(
        &service,
        Command::SendMessage {
            session_id: session.id.clone(),
            text: "durable input".into(),
        },
    )
    .await
    else {
        panic!("Run");
    };
    // Restore the durable state at the crash boundary: send attempted, no terminal commit.
    let mut loaded = store
        .read(&[
            ControlFilter::id(ControlRecordKind::Run, &run.id),
            ControlFilter::id(ControlRecordKind::Session, &session.id),
        ])
        .await
        .unwrap();
    for record in &mut loaded.records {
        if record.kind == ControlRecordKind::Run {
            assert_eq!(record.value["codex_input"]["state"], "send_unknown");
            assert_eq!(record.value["codex_input"]["text"], "durable input");
            record.value["status"] = json!("running");
            record.value["phase"] = json!("calling_agent");
            record.value["error"] = json!(null);
        } else {
            record.value["active_run_id"] = json!(run.id);
        }
    }
    store
        .apply(
            loaded.revision,
            loaded.records.into_iter().map(ControlChange::Put).collect(),
            Vec::new(),
        )
        .await
        .unwrap();
    let restarted = LocalControlService::new(
        Arc::new(ait_workspace_local::LocalProjectWorkspace::default()),
        store,
    )
    .with_codex_history_source(fixture.clone())
    .with_codex_thread_writer(fixture.clone());
    let recovered = restarted.recover_interrupted_runs().await.unwrap();
    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0].id, run.id);
    assert_eq!(recovered[0].status, "completed");
    assert_eq!(fixture.writes.lock().unwrap().len(), 1);
    let after = messages(&restarted).await;
    assert_eq!(
        after
            .iter()
            .filter(|m| m.text.as_deref() == Some("durable input"))
            .count(),
        1
    );
}

#[tokio::test]
async fn later_history_sync_preserves_a_local_resource_limit_outcome() {
    let (_directory, fixture, service, session) = setup().await;
    fixture
        .snapshot
        .lock()
        .unwrap()
        .metadata
        .insert("limited".into(), json!(true));
    let CommandResult::Run(run) = ok(
        &service,
        Command::SendMessage {
            session_id: session.id,
            text: "bounded output".into(),
        },
    )
    .await
    else {
        panic!("Run");
    };
    assert_eq!(run.status, "limit_exceeded");
    let error = run.error.clone().unwrap();
    assert!(error.message.contains("observed 8388609, limit 8388608"));
    ok(&service, sync()).await;
    let CommandResult::Run(reconciled) = ok(&service, Command::GetRun { run_id: run.id }).await
    else {
        panic!("Run");
    };
    assert_eq!(reconciled.status, "limit_exceeded");
    assert_eq!(reconciled.error, Some(error));
    assert_eq!(
        reconciled.error.unwrap().code,
        ait_domain::ErrorCode::RunLimitExceeded
    );
    assert_eq!(
        messages(&service)
            .await
            .iter()
            .filter(|message| message.text.as_deref() == Some("bounded output"))
            .count(),
        1
    );
    assert_eq!(fixture.writes.lock().unwrap().len(), 1);
}
