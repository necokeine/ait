//! Name-only registration through the production application, Git, and `SQLite` boundaries.

use std::{
    fs,
    path::{Path, PathBuf},
    process::Command as Git,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use ait_application::LocalControlService;
use ait_contracts::{Command, CommandResult, Response};
use ait_domain::{DomainError, ErrorCode};
use ait_ports::{
    ControlChange, ControlFilter, ControlRead, ControlRecordKind, ControlStore, ControlStoreError,
    DurableEvent, DurableEventPage, EventBounds, PendingEvent, ProgressCheckpoint,
    ProjectDirectoryCreator,
};
use ait_project_local::DocumentsProjectDirectory;
use ait_storage_sqlite::SqliteControlStore;
use async_trait::async_trait;
use tempfile::TempDir;

fn creator(path: &Path) -> DocumentsProjectDirectory {
    let path = path.to_path_buf();
    DocumentsProjectDirectory::with_resolver(move || Some(path.clone()))
}

fn registration(id: &str, name: &str, workdir: Option<String>) -> Command {
    Command::RegisterProject {
        id: id.into(),
        name: name.into(),
        workdir,
        repo_url: None,
    }
}

fn git(path: &Path, arguments: &[&str]) -> String {
    let output = Git::new("git")
        .arg("-C")
        .arg(path)
        .args(arguments)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

async fn records(store: &dyn ControlStore) -> ControlRead {
    store
        .read(&[
            ControlFilter::all(ControlRecordKind::Project),
            ControlFilter::all(ControlRecordKind::Message),
        ])
        .await
        .unwrap()
}

fn failure(response: Response, expected: ErrorCode) -> String {
    assert!(!response.ok, "{response:?}");
    let error = response.error.unwrap();
    assert_eq!(error.code, expected, "{}", error.message);
    assert!(!error.retryable);
    error.message
}

#[tokio::test]
async fn name_only_creates_git_head_and_atomic_project_root_and_rejects_duplicates() {
    let root = TempDir::new().unwrap();
    let store = Arc::new(SqliteControlStore::in_memory().unwrap());
    let service = LocalControlService::new(store.clone())
        .with_project_directory_creator(Arc::new(creator(root.path())));
    let response = service
        .execute(registration("p", "中文 project", None))
        .await;
    assert!(response.ok, "{response:?}");
    let Some(CommandResult::Project(project)) = response.result else {
        panic!("missing project")
    };
    let path = root.path().join("中文 project");
    assert_eq!(Path::new(&project.workdir), path.canonicalize().unwrap());
    assert_eq!(project.base_commit, git(&path, &["rev-parse", "HEAD"]));
    assert_eq!(
        git(&path, &["rev-parse", "--show-toplevel"]),
        project.workdir
    );
    assert_eq!(git(&path, &["rev-list", "--count", "HEAD"]), "1");
    assert!(git(&path, &["status", "--porcelain=v1"]).is_empty());
    assert!(git(&path, &["ls-tree", "--name-only", "HEAD"]).is_empty());
    let before = records(store.as_ref()).await;
    assert_eq!(before.records.len(), 2);
    let events = store.replay(0, 100).await.unwrap();
    fs::write(path.join("keep"), "member data").unwrap();
    let conflict = failure(
        service
            .execute(registration("other", "中文 project", None))
            .await,
        ErrorCode::ProjectPathAlreadyExists,
    );
    assert!(conflict.contains(&project.workdir));
    assert_eq!(git(&path, &["rev-parse", "HEAD"]), project.base_commit);
    assert_eq!(
        fs::read_to_string(path.join("keep")).unwrap(),
        "member data"
    );
    assert_eq!(records(store.as_ref()).await, before);
    assert_eq!(store.replay(0, 100).await.unwrap(), events);
    failure(
        service.execute(registration("p", "unused", None)).await,
        ErrorCode::InvalidProject,
    );
    assert!(!root.path().join("unused").exists());
}

#[tokio::test]
async fn explicit_workdirs_keep_existing_semantics_and_never_resolve_documents() {
    let root = TempDir::new().unwrap();
    let service = LocalControlService::new(Arc::new(SqliteControlStore::in_memory().unwrap()))
        .with_project_directory_creator(Arc::new(DocumentsProjectDirectory::with_resolver(|| {
            panic!("explicit path resolved Documents")
        })));
    fs::write(root.path().join("keep"), "existing content").unwrap();
    let response = service
        .execute(registration(
            "explicit",
            "Display/name",
            Some(root.path().display().to_string()),
        ))
        .await;
    assert!(response.ok, "{response:?}");
    assert_eq!(
        fs::read_to_string(root.path().join("keep")).unwrap(),
        "existing content"
    );
    assert!(root.path().join(".git").is_dir());
    failure(
        service
            .execute(registration(
                "missing",
                "Missing",
                Some(root.path().join("missing").display().to_string()),
            ))
            .await,
        ErrorCode::ProjectPathNotFound,
    );
    failure(
        service
            .execute(registration("empty", "Empty", Some(String::new())))
            .await,
        ErrorCode::ProjectPathNotFound,
    );
    assert!(!root.path().join("missing").exists());
}

#[tokio::test]
async fn invalid_registration_and_unavailable_documents_do_not_write_state() {
    let root = TempDir::new().unwrap();
    let store = Arc::new(SqliteControlStore::in_memory().unwrap());
    let service = LocalControlService::new(store.clone())
        .with_project_directory_creator(Arc::new(creator(root.path())));
    let before = records(store.as_ref()).await;
    for (id, name) in [
        ("", "valid"),
        ("p", ""),
        ("p", " "),
        ("p", "../escape"),
        ("p", "a\\b"),
    ] {
        failure(
            service.execute(registration(id, name, None)).await,
            ErrorCode::InvalidProject,
        );
    }
    failure(
        service
            .execute(Command::RegisterProject {
                id: "p".into(),
                name: "valid".into(),
                workdir: None,
                repo_url: Some(" ".into()),
            })
            .await,
        ErrorCode::InvalidProject,
    );
    let unavailable = LocalControlService::new(store.clone()).with_project_directory_creator(
        Arc::new(DocumentsProjectDirectory::with_resolver(|| None)),
    );
    failure(
        unavailable.execute(registration("p", "valid", None)).await,
        ErrorCode::ProjectDefaultDirectoryUnavailable,
    );
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 0);
    assert_eq!(records(store.as_ref()).await, before);
    assert!(store.replay(0, 100).await.unwrap().is_empty());
}

struct GitFailure {
    directory: DocumentsProjectDirectory,
    fail_head: bool,
}

impl ProjectDirectoryCreator for GitFailure {
    fn create_workdir(&self, name: &str) -> Result<PathBuf, DomainError> {
        let path = self.directory.create_workdir(name)?;
        if self.fail_head {
            git(&path, &["init", "--quiet"]);
            fs::write(path.join(".git/index.lock"), "lock fixture").unwrap();
        } else {
            fs::write(path.join(".git"), "invalid git file fixture").unwrap();
        }
        fs::write(path.join("concurrent-member-file"), "do not remove").unwrap();
        Ok(path)
    }
}

#[tokio::test]
async fn git_init_and_head_failures_preserve_files_without_registering() {
    for fail_head in [false, true] {
        let root = TempDir::new().unwrap();
        let store = Arc::new(SqliteControlStore::in_memory().unwrap());
        let service = LocalControlService::new(store.clone()).with_project_directory_creator(
            Arc::new(GitFailure {
                directory: creator(root.path()),
                fail_head,
            }),
        );
        let before = records(store.as_ref()).await;
        let code = if fail_head {
            ErrorCode::ProjectGitHeadUnavailable
        } else {
            ErrorCode::ProjectGitInitFailed
        };
        let message = failure(
            service.execute(registration("p", "retained", None)).await,
            code,
        );
        let path = root.path().join("retained");
        assert!(message.contains("Directory retained at"));
        assert!(message.contains(path.canonicalize().unwrap().to_str().unwrap()));
        assert_eq!(
            fs::read_to_string(path.join("concurrent-member-file")).unwrap(),
            "do not remove"
        );
        assert_eq!(records(store.as_ref()).await, before);
        assert!(store.replay(0, 100).await.unwrap().is_empty());
    }
}

struct StoreFault {
    inner: SqliteControlStore,
    conflicts: AtomicUsize,
    fail: bool,
}

#[async_trait]
impl ControlStore for StoreFault {
    async fn read(&self, filters: &[ControlFilter]) -> Result<ControlRead, ControlStoreError> {
        self.inner.read(filters).await
    }
    async fn apply(
        &self,
        revision: u64,
        changes: Vec<ControlChange>,
        events: Vec<PendingEvent>,
    ) -> Result<u64, ControlStoreError> {
        if self
            .conflicts
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
            .is_ok()
        {
            return Err(ControlStoreError::Conflict);
        }
        if self.fail {
            return Err(ControlStoreError::Other("injected storage failure".into()));
        }
        self.inner.apply(revision, changes, events).await
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

#[tokio::test]
async fn cas_retry_reuses_only_this_requests_allocation() {
    let root = TempDir::new().unwrap();
    let store = Arc::new(StoreFault {
        inner: SqliteControlStore::in_memory().unwrap(),
        conflicts: AtomicUsize::new(1),
        fail: false,
    });
    let service = LocalControlService::new(store.clone())
        .with_project_directory_creator(Arc::new(creator(root.path())));
    let response = service.execute(registration("p", "retried", None)).await;
    assert!(response.ok, "{response:?}");
    assert_eq!(
        git(
            &root.path().join("retried"),
            &["rev-list", "--count", "HEAD"]
        ),
        "1"
    );
    assert_eq!(records(store.as_ref()).await.records.len(), 2);
    assert_eq!(store.replay(0, 100).await.unwrap().len(), 1);
    failure(
        service
            .execute(registration("other", "retried", None))
            .await,
        ErrorCode::ProjectPathAlreadyExists,
    );
}

#[tokio::test]
async fn persistence_failure_and_exhausted_cas_retain_git_without_half_registration() {
    for conflicts in [0, 4] {
        let root = TempDir::new().unwrap();
        let store = Arc::new(StoreFault {
            inner: SqliteControlStore::in_memory().unwrap(),
            conflicts: AtomicUsize::new(conflicts),
            fail: true,
        });
        let before = records(store.as_ref()).await;
        let service = LocalControlService::new(store.clone())
            .with_project_directory_creator(Arc::new(creator(root.path())));
        let response = service.execute(registration("p", "retained", None)).await;
        let error = response.error.unwrap();
        assert!(!error.retryable);
        assert!(error.message.contains("Directory retained at"));
        assert_eq!(
            git(
                &root.path().join("retained"),
                &["rev-list", "--count", "HEAD"]
            ),
            "1"
        );
        assert_eq!(records(store.as_ref()).await, before);
        assert!(store.replay(0, 100).await.unwrap().is_empty());
    }
}
