//! Project-scoped discovery uses the same ownership rules as native import.

use std::{
    path::Path,
    sync::atomic::{AtomicUsize, Ordering},
};

use super::*;

#[derive(Debug)]
struct CatalogFixture {
    threads: Mutex<Vec<CodexThreadSnapshot>>,
    lists: AtomicUsize,
}

#[async_trait]
impl CodexHistorySource for CatalogFixture {
    async fn list_threads(
        &self,
        sources: &[CodexThreadSourceKind],
    ) -> Result<Vec<CodexThreadSnapshot>, DomainError> {
        self.lists.fetch_add(1, Ordering::Relaxed);
        assert!(sources.contains(&CodexThreadSourceKind::AppServer));
        assert!(sources.contains(&CodexThreadSourceKind::Cli));
        Ok(self.threads.lock().unwrap().clone())
    }

    async fn read_thread(&self, id: &str) -> Result<CodexThreadSnapshot, DomainError> {
        Ok(self
            .threads
            .lock()
            .unwrap()
            .iter()
            .find(|thread| thread.id == id)
            .unwrap()
            .clone())
    }
}

async fn setup(root: &Path, paths: &[(&str, &Path)]) -> (LocalControlService, Arc<CatalogFixture>) {
    let fixture = Arc::new(CatalogFixture {
        threads: Mutex::new(
            paths
                .iter()
                .map(|(id, cwd)| {
                    let mut thread = snapshot(cwd.display().to_string());
                    thread.id = (*id).into();
                    thread
                })
                .collect(),
        ),
        lists: AtomicUsize::new(0),
    });
    let service = LocalControlService::new(
        Arc::new(ait_workspace_local::LocalProjectWorkspace::default()),
        Arc::new(SqliteControlStore::in_memory().unwrap()),
    )
    .with_codex_history_source(fixture.clone());
    register_binding(&service, root).await;
    (service, fixture)
}

async fn list(
    service: &LocalControlService,
    project: Option<&str>,
) -> Vec<ait_contracts::CodexThreadView> {
    let CommandResult::CodexThreads(threads) = ok(
        service,
        Command::ListCodexThreads {
            provider_id: "builtin-codex".into(),
            project_id: project.map(str::to_owned),
        },
    )
    .await
    else {
        panic!("expected threads")
    };
    threads
}

async fn register_project(service: &LocalControlService, id: &str, root: &Path) {
    ok(
        service,
        Command::RegisterProject {
            id: id.into(),
            name: id.into(),
            workdir: Some(root.display().to_string()),
            repo_url: None,
        },
    )
    .await;
}

#[tokio::test]
async fn filters_by_directory_boundary_and_rejects_ambiguous_nested_projects() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("repo");
    let child = root.join("child");
    let sibling = directory.path().join("repo-other");
    std::fs::create_dir_all(&child).unwrap();
    std::fs::create_dir_all(&sibling).unwrap();
    let missing = root.join("missing");
    let (service, fixture) = setup(
        &root,
        &[
            ("root", &root),
            ("child", &child),
            ("sibling", &sibling),
            ("missing", &missing),
        ],
    )
    .await;
    fixture.threads.lock().unwrap()[0].archived = true;
    assert_eq!(list(&service, None).await.len(), 4);
    let matching = list(&service, Some("project")).await;
    assert_eq!(
        matching
            .iter()
            .map(|thread| thread.thread_id.as_str())
            .collect::<Vec<_>>(),
        ["child", "root"]
    );
    assert!(matching[1].archived);
    register_project(&service, "nested", &child).await;
    assert_eq!(list(&service, Some("project")).await[0].thread_id, "root");
    assert_eq!(list(&service, Some("project")).await.len(), 1);
    assert!(list(&service, Some("nested")).await.is_empty());
}

#[tokio::test]
async fn existing_binding_wins_over_cwd_changes_and_does_not_leak_to_other_project() {
    let root = tempfile::tempdir().unwrap();
    let other = tempfile::tempdir().unwrap();
    let (service, fixture) = setup(root.path(), &[("bound", root.path())]).await;
    register_project(&service, "other", other.path()).await;
    let CommandResult::Session(session) = ok(
        &service,
        Command::SyncCodexThread {
            provider_id: "builtin-codex".into(),
            thread_id: "bound".into(),
            project_id: "project".into(),
            agent_id: "agent".into(),
        },
    )
    .await
    else {
        panic!("expected session")
    };
    fixture.threads.lock().unwrap()[0].cwd = other.path().display().to_string();
    let matching = list(&service, Some("project")).await;
    assert_eq!(matching.len(), 1);
    assert_eq!(matching[0].session_id.as_deref(), Some(session.id.as_str()));
    assert_eq!(matching[0].project_id.as_deref(), Some("project"));
    assert!(list(&service, Some("other")).await.is_empty());
}

#[tokio::test]
async fn unknown_project_is_rejected_before_calling_codex() {
    let root = tempfile::tempdir().unwrap();
    let (service, fixture) = setup(root.path(), &[]).await;
    let response = service
        .execute(Command::ListCodexThreads {
            provider_id: "builtin-codex".into(),
            project_id: Some("missing".into()),
        })
        .await;
    assert_eq!(
        response.error.unwrap().code,
        ait_domain::ErrorCode::InvalidProject
    );
    assert_eq!(fixture.lists.load(Ordering::Relaxed), 0);
}

#[cfg(unix)]
#[tokio::test]
async fn resolves_symlinks_before_determining_project_ownership() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("root");
    let outside = directory.path().join("outside");
    let alias = directory.path().join("alias");
    let escape = root.join("escape");
    std::fs::create_dir(&root).unwrap();
    std::fs::create_dir(&outside).unwrap();
    std::os::unix::fs::symlink(&root, &alias).unwrap();
    std::os::unix::fs::symlink(&outside, &escape).unwrap();
    let (service, _) = setup(&root, &[("alias", &alias), ("escape", &escape)]).await;
    let matching = list(&service, Some("project")).await;
    assert_eq!(matching.len(), 1);
    assert_eq!(matching[0].thread_id, "alias");
}
