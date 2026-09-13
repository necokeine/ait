//! Local Project facts and resource ownership, independent of transport and storage.

use ait_domain::DomainError;
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

/// Stable, clean HEAD/index snapshot captured with checks on both sides of status.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GitBaseline {
    /// Full commit object identity.
    pub commit: String,
    /// Full index tree object identity, equal to the commit tree.
    pub index_tree: String,
}

/// Canonical filesystem observations; the application decides authorization.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkspacePathFacts {
    /// Canonical Project root, which must be a UTF-8 directory.
    pub canonical_root: PathBuf,
    /// Canonical nearest existing ancestor of the requested destination.
    /// Dangling symlinks and unresolvable/non-UTF-8 paths must return an error.
    pub canonical_existing: PathBuf,
}

/// Opaque advisory write lease. Clones retain ownership until the last drop.
/// Implementations must preserve exclusion across canonical aliases and processes.
pub trait WorkspaceLease: Send + Sync {
    /// Canonical Project whose workspace this lease protects.
    fn canonical_root(&self) -> &Path;
}

/// Async infrastructure boundary for control-plane Project operations.
///
/// Implementations bound blocking concurrency and Git duration. Dropping a future
/// cancels queued work and requests cancellation of started work; started blocking
/// calls retain their permits and any supplied lease until they stop. Filesystem
/// syscalls cannot be forcibly interrupted. Partial initialization/worktrees are
/// retained for inspection, never automatically reset/cleaned or rolled back.
/// No method grants permission, moves a Session, publishes a Run, or writes storage.
#[async_trait::async_trait]
pub trait ProjectWorkspace: Send + Sync {
    /// Prepare and verify an exact canonical Git root (nested roots are allowed).
    /// # Errors
    /// Stable Project path/init errors; non-UTF-8 paths fail closed.
    async fn prepare_git_root(&self, path: &Path) -> Result<PathBuf, DomainError>;
    /// Read HEAD, creating only an empty initial commit for an unstaged unborn repo.
    /// # Errors
    /// Stable HEAD errors, including a staged unborn index.
    async fn ensure_git_head(&self, path: &Path) -> Result<String, DomainError>;
    /// Read a full HEAD object identity; `None` denotes an unborn HEAD.
    /// # Errors
    /// Stable HEAD errors for failed inspection or malformed output.
    async fn git_head(&self, path: &Path) -> Result<Option<String>, DomainError>;
    /// Compare HEAD and index before/after status, then require index == HEAD tree.
    /// # Errors
    /// Dirty, unborn, moved HEAD/index, or unavailable Git errors.
    async fn clean_baseline(&self, path: &Path) -> Result<GitBaseline, DomainError>;
    /// Return the symbolic HEAD, or `None` for detached HEAD.
    /// # Errors
    /// Stable HEAD errors, including invalid Git output.
    async fn symbolic_head(&self, path: &Path) -> Result<Option<String>, DomainError>;
    /// Resolve the absolute Git metadata directory.
    /// # Errors
    /// Stable HEAD errors for unavailable or unresolvable Git metadata.
    async fn git_dir(&self, path: &Path) -> Result<PathBuf, DomainError>;
    /// Acquire canonical in-process admission and a nonblocking cross-process lock.
    /// # Errors
    /// Stable Project path or retryable workspace-busy failures.
    async fn acquire_lease(&self, path: &Path) -> Result<Arc<dyn WorkspaceLease>, DomainError>;
    /// Ensure a linked Session worktree, returning whether it was created.
    /// Only a newly created manager-owned worktree may be populated at `baseline`.
    /// An existing directory must be its own Git root and must never be reset.
    /// The supplied lease must protect `primary`; absent leases are acquired here.
    /// # Errors
    /// Stable path/Git/Session errors; partial worktrees are retained on failure.
    async fn ensure_session_worktree(
        &self,
        primary: &Path,
        worktree: &Path,
        baseline: &str,
        lease: Option<Arc<dyn WorkspaceLease>>,
    ) -> Result<bool, DomainError>;
    /// Observe canonical root and nearest existing ancestor without granting access.
    /// # Errors
    /// Unresolvable paths, dangling symlinks, or non-UTF-8 paths fail closed.
    async fn path_facts(
        &self,
        root: &Path,
        destination: &Path,
    ) -> Result<WorkspacePathFacts, DomainError>;
}
