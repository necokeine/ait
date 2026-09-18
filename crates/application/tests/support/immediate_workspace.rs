//! Ready workspace facts for the application submission/progress latency contract.
//! Native Git, path validation and OS lease behavior have separate adapter tests.
use ait_domain::DomainError;
use ait_workspace::{GitBaseline, ProjectWorkspace, WorkspaceLease, WorkspacePathFacts};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

pub struct ImmediateWorkspace;
struct Lease(PathBuf);
impl WorkspaceLease for Lease {
    fn canonical_root(&self) -> &Path {
        &self.0
    }
}

#[async_trait::async_trait]
impl ProjectWorkspace for ImmediateWorkspace {
    async fn prepare_git_root(
        &self,
        path: &Path,
        expected: Option<&Path>,
    ) -> Result<PathBuf, DomainError> {
        assert_eq!(expected, Some(path));
        Ok(path.to_owned())
    }
    async fn verify_git_root(&self, _: &Path) -> Result<(), DomainError> {
        Ok(())
    }
    async fn ensure_git_head(&self, _: &Path) -> Result<String, DomainError> {
        Ok("a".repeat(40))
    }
    async fn git_head(&self, _: &Path) -> Result<Option<String>, DomainError> {
        Ok(Some("a".repeat(40)))
    }
    async fn clean_baseline(&self, _: &Path) -> Result<GitBaseline, DomainError> {
        Ok(GitBaseline {
            commit: "a".repeat(40),
            index_tree: "b".repeat(40),
        })
    }
    async fn symbolic_head(&self, _: &Path) -> Result<Option<String>, DomainError> {
        Ok(None)
    }
    async fn git_dir(&self, _: &Path) -> Result<PathBuf, DomainError> {
        panic!("unexpected Git metadata request")
    }
    async fn acquire_lease(&self, root: &Path) -> Result<Arc<dyn WorkspaceLease>, DomainError> {
        Ok(Arc::new(Lease(root.to_owned())))
    }
    async fn ensure_session_worktree(
        &self,
        _: &Path,
        _: &Path,
        _: &str,
        _: Option<Arc<dyn WorkspaceLease>>,
    ) -> Result<bool, DomainError> {
        Ok(false)
    }
    async fn path_facts(
        &self,
        root: &Path,
        destination: &Path,
    ) -> Result<WorkspacePathFacts, DomainError> {
        Ok(WorkspacePathFacts {
            canonical_root: root.to_owned(),
            canonical_existing: destination.to_owned(),
        })
    }
}
