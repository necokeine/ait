//! Native Codex history synchronization and writable continuation coverage.
#![allow(clippy::pedantic)]

use std::sync::{Arc, Mutex};

use ait_application::LocalControlService;
use ait_contracts::{AgentConfiguration, Command, CommandResult};
use ait_domain::{DomainError, SessionSource};
use ait_ports::{
    CodexHistorySource, CodexItemsView, CodexThreadInvocation, CodexThreadSnapshot,
    CodexThreadSourceKind, CodexThreadWriter, CodexTurnSnapshot, WorkspaceAgent,
    WorkspaceAgentInvocation, WorkspaceAgentResponse, WorkspaceProgressReporter,
};
use ait_storage_sqlite::SqliteControlStore;
use async_trait::async_trait;
use serde_json::json;

#[derive(Debug)]
struct UnusedWorkspaceAgent;

#[async_trait]
impl WorkspaceAgent for UnusedWorkspaceAgent {
    async fn invoke(
        &self,
        _request: WorkspaceAgentInvocation,
    ) -> Result<WorkspaceAgentResponse, DomainError> {
        panic!("imported NativeCwd Session must not use the managed workspace agent")
    }
}

#[derive(Debug)]
struct NativeCodexFixture {
    snapshot: Mutex<CodexThreadSnapshot>,
    writes: Mutex<Vec<CodexThreadInvocation>>,
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

#[async_trait]
impl CodexThreadWriter for NativeCodexFixture {
    async fn continue_thread(
        &self,
        request: CodexThreadInvocation,
        _progress: Arc<dyn WorkspaceProgressReporter>,
    ) -> Result<WorkspaceAgentResponse, DomainError> {
        self.writes.lock().unwrap().push(request);
        Ok(WorkspaceAgentResponse {
            assistant_text: "continued native Thread".into(),
            commit_id: None,
            operations: Vec::new(),
            output_items: Vec::new(),
        })
    }
}

fn snapshot(cwd: String) -> CodexThreadSnapshot {
    CodexThreadSnapshot {
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
        snapshot: Mutex::new(snapshot(cwd.display().to_string())),
        writes: Mutex::default(),
    });
    let store = Arc::new(SqliteControlStore::in_memory().unwrap());
    let service = LocalControlService::with_workspace_agent(
        Arc::new(ait_workspace_local::LocalProjectWorkspace::default()),
        store,
        Arc::new(UnusedWorkspaceAgent),
    )
    .with_codex_history_source(fixture.clone())
    .with_codex_thread_writer(fixture.clone());

    register_binding(&service, &cwd).await;

    let CommandResult::CodexThreads(threads) = ok(
        &service,
        Command::ListCodexThreads {
            provider_id: "builtin-codex".into(),
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
        assert_eq!(writes[0].thread_id, "native-thread");
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
}

#[tokio::test]
async fn imported_thread_rejects_input_while_provider_reports_external_activity() {
    let directory = tempfile::tempdir().unwrap();
    let cwd = directory.path().canonicalize().unwrap();
    let mut active = snapshot(cwd.display().to_string());
    active.status = json!({"type": "active"});
    let fixture = Arc::new(NativeCodexFixture {
        snapshot: Mutex::new(active),
        writes: Mutex::default(),
    });
    let service = LocalControlService::with_workspace_agent(
        Arc::new(ait_workspace_local::LocalProjectWorkspace::default()),
        Arc::new(SqliteControlStore::in_memory().unwrap()),
        Arc::new(UnusedWorkspaceAgent),
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

    let CommandResult::Run(run) = ok(
        &service,
        Command::SendMessage {
            session_id: session.id,
            text: "must not be sent".into(),
        },
    )
    .await
    else {
        panic!("expected persisted failed Run")
    };

    assert_eq!(run.status, "failed");
    assert_eq!(
        run.error.unwrap().code,
        ait_domain::ErrorCode::CodexThreadActiveElsewhere
    );
    assert!(fixture.writes.lock().unwrap().is_empty());
}
