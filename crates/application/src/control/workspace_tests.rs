//! Pure Project facts at application admission, authorization and transaction boundaries.
#![allow(clippy::pedantic)]
use super::LocalControlService;
use ait_contracts::{AgentConfiguration, Command};
use ait_domain::{DomainError, ErrorCode};
use ait_ports::ControlStore;
use ait_storage_sqlite::SqliteControlStore;
use ait_workspace::{GitBaseline, ProjectWorkspace, WorkspaceLease, WorkspacePathFacts};
use async_trait::async_trait;
use std::path::{Path, PathBuf};

pub(super) struct FakeWorkspace {
    pub(super) trace: Arc<Mutex<Vec<&'static str>>>,
    pub(super) facts: Mutex<Result<WorkspacePathFacts, DomainError>>,
    dirty: AtomicBool,
}

impl Default for FakeWorkspace {
    fn default() -> Self {
        Self {
            trace: Arc::default(),
            dirty: AtomicBool::new(false),
            facts: Mutex::new(Ok(WorkspacePathFacts {
                canonical_root: if cfg!(windows) {
                    "C:/real/project"
                } else {
                    "/real/project"
                }
                .into(),
                canonical_existing: if cfg!(windows) {
                    "C:/real/project"
                } else {
                    "/real/project"
                }
                .into(),
            })),
        }
    }
}

struct FakeLease(PathBuf);
impl WorkspaceLease for FakeLease {
    fn canonical_root(&self) -> &Path {
        &self.0
    }
}

#[async_trait]
impl ProjectWorkspace for FakeWorkspace {
    async fn prepare_git_root(
        &self,
        path: &Path,
        _: Option<&Path>,
    ) -> Result<PathBuf, DomainError> {
        self.trace.lock().unwrap().push("prepare");
        Ok(path.to_owned())
    }
    async fn verify_git_root(&self, _: &Path) -> Result<(), DomainError> {
        self.trace.lock().unwrap().push("verify");
        Ok(())
    }
    async fn ensure_git_head(&self, _: &Path) -> Result<String, DomainError> {
        self.trace.lock().unwrap().push("head");
        Ok("a".repeat(40))
    }
    async fn git_head(&self, _: &Path) -> Result<Option<String>, DomainError> {
        Ok(Some("a".repeat(40)))
    }
    async fn clean_baseline(&self, _: &Path) -> Result<GitBaseline, DomainError> {
        self.trace.lock().unwrap().push("baseline");
        if self.dirty.load(Ordering::SeqCst) {
            return Err(DomainError::invariant(ErrorCode::ProjectGitDirty, "dirty"));
        }
        Ok(GitBaseline {
            commit: "a".repeat(40),
            index_tree: "b".repeat(40),
        })
    }
    async fn symbolic_head(&self, _: &Path) -> Result<Option<String>, DomainError> {
        Ok(None)
    }
    async fn git_dir(&self, _: &Path) -> Result<PathBuf, DomainError> {
        panic!("not an application fact request")
    }
    async fn acquire_lease(&self, path: &Path) -> Result<Arc<dyn WorkspaceLease>, DomainError> {
        self.trace.lock().unwrap().push("lease");
        Ok(Arc::new(FakeLease(path.to_owned())))
    }
    async fn ensure_session_worktree(
        &self,
        _: &Path,
        _: &Path,
        _: &str,
        lease: Option<Arc<dyn WorkspaceLease>>,
    ) -> Result<bool, DomainError> {
        assert!(
            lease.is_some(),
            "application must retain workspace ownership"
        );
        self.trace.lock().unwrap().push("worktree");
        Ok(false)
    }
    async fn path_facts(&self, _: &Path, _: &Path) -> Result<WorkspacePathFacts, DomainError> {
        self.trace.lock().unwrap().push("facts");
        self.facts.lock().unwrap().clone()
    }
}

fn registration() -> Command {
    Command::RegisterProject {
        id: "p".into(),
        name: "Project".into(),
        workdir: Some("/alias/project".into()),
        repo_url: None,
    }
}

#[tokio::test]
async fn registration_facts_are_verified_again_on_cas_conflict_before_persisting() {
    let workspace = Arc::new(FakeWorkspace::default());
    let store = Arc::new(RecordingStore {
        inner: SqliteControlStore::in_memory().unwrap(),
        trace: workspace.trace.clone(),
        conflict: AtomicBool::new(true),
    });
    let service = LocalControlService::new(workspace.clone(), store);
    let result = service.execute(registration()).await;
    assert!(result.ok, "{:?}", result.error);
    assert_eq!(
        *workspace.trace.lock().unwrap(),
        [
            "facts", "prepare", "head", "verify", "persist", "verify", "persist"
        ]
    );
    workspace.trace.lock().unwrap().clear();
    assert!(!service.execute(registration()).await.ok);
    assert!(
        workspace.trace.lock().unwrap().is_empty(),
        "invalid identity must fail before I/O"
    );
}

#[tokio::test]
async fn missing_native_writer_never_persists_message_run_or_session_changes() {
    let workspace = Arc::new(FakeWorkspace::default());
    let store = Arc::new(RecordingStore {
        inner: SqliteControlStore::in_memory().unwrap(),
        trace: workspace.trace.clone(),
        conflict: AtomicBool::new(false),
    });
    let service = LocalControlService::new(workspace.clone(), store.clone());
    for command in [
        registration(),
        Command::RegisterAgent {
            id: "agent".into(),
            name: "Agent".into(),
            config: AgentConfiguration {
                provider_id: "builtin-codex".into(),
                model: "gpt-5.6-sol".into(),
                reasoning_effort: Some("low".into()),
                system_prompt: None,
            },
        },
        Command::CreateSession {
            id: "session".into(),
            project_id: "p".into(),
            agent_id: "agent".into(),
            at_message_id: None,
        },
    ] {
        let response = service.execute(command).await;
        assert!(response.ok, "{:?}", response.error);
    }
    let filters = [
        ait_ports::ControlFilter::All(ait_ports::ControlRecordKind::Message),
        ait_ports::ControlFilter::All(ait_ports::ControlRecordKind::Run),
        ait_ports::ControlFilter::All(ait_ports::ControlRecordKind::Session),
    ];
    let before = store.read(&filters).await.unwrap();
    workspace.trace.lock().unwrap().clear();
    workspace.dirty.store(true, Ordering::SeqCst);
    let response = service
        .execute(Command::SendMessage {
            session_id: "session".into(),
            text: "hello".into(),
        })
        .await;
    assert_eq!(
        response.error.unwrap().code,
        ErrorCode::CodexThreadCapabilityUnsupported
    );
    assert!(!workspace.trace.lock().unwrap().contains(&"baseline"));
    assert_eq!(store.read(&filters).await.unwrap(), before);
}
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

struct RecordingStore {
    inner: SqliteControlStore,
    trace: Arc<Mutex<Vec<&'static str>>>,
    conflict: AtomicBool,
}
#[async_trait]
impl ControlStore for RecordingStore {
    async fn read(
        &self,
        filters: &[ait_ports::ControlFilter],
    ) -> Result<ait_ports::ControlRead, ait_ports::ControlStoreError> {
        self.inner.read(filters).await
    }
    async fn replay(
        &self,
        after: u64,
        limit: usize,
    ) -> Result<Vec<ait_ports::DurableEvent>, ait_ports::ControlStoreError> {
        self.inner.replay(after, limit).await
    }
    async fn event_bounds(&self) -> Result<ait_ports::EventBounds, ait_ports::ControlStoreError> {
        self.inner.event_bounds().await
    }
    async fn replay_page(
        &self,
        after: u64,
        limit: usize,
    ) -> Result<ait_ports::DurableEventPage, ait_ports::ControlStoreError> {
        self.inner.replay_page(after, limit).await
    }
    async fn save_progress(
        &self,
        checkpoint: ait_ports::ProgressCheckpoint,
        events: Vec<ait_ports::PendingEvent>,
    ) -> Result<(), ait_ports::ControlStoreError> {
        self.inner.save_progress(checkpoint, events).await
    }
    async fn load_progress(
        &self,
        project_id: &str,
    ) -> Result<Vec<ait_ports::ProgressCheckpoint>, ait_ports::ControlStoreError> {
        self.inner.load_progress(project_id).await
    }
    async fn clear_progress(&self, run_id: &str) -> Result<(), ait_ports::ControlStoreError> {
        self.inner.clear_progress(run_id).await
    }
    async fn apply(
        &self,
        revision: u64,
        changes: Vec<ait_ports::ControlChange>,
        events: Vec<ait_ports::PendingEvent>,
    ) -> Result<u64, ait_ports::ControlStoreError> {
        self.trace.lock().unwrap().push("persist");
        if self.conflict.swap(false, Ordering::SeqCst) {
            return Err(ait_ports::ControlStoreError::Conflict);
        }
        self.inner.apply(revision, changes, events).await
    }
}
