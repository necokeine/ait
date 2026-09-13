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
/// cancels queued work and releases its captured leases/permits without waiting
/// for a blocking thread, and requests cancellation of started work; started blocking
/// calls retain their permits and any supplied lease until they stop. Filesystem
/// syscalls cannot be forcibly interrupted. Partial initialization/worktrees are
/// retained for inspection, never automatically reset/cleaned or rolled back.
/// Each public call shares one deadline across admission and nested phases (including
/// implicit lease acquisition). A timeout uses the operation's Project error code
/// and `details.reason = "timeout"`. If a business mutation may have started, errors
/// include `details.retained_paths` entries with `path` and `state`, repeat them in
/// the message for API callers, and disable blind retries (`retryable = false`).
/// A `*_started` state denotes an uncertain partial result, not confirmed success.
/// No method grants permission, moves a Session, publishes a Run, or writes storage.
#[async_trait::async_trait]
pub trait ProjectWorkspace: Send + Sync {
    /// Prepare and verify an exact canonical Git root (nested roots are allowed).
    /// If supplied, `expected_root` must still be the canonical target before mutation.
    /// # Errors
    /// Stable Project path/init errors; non-UTF-8 paths fail closed. A rebound
    /// expected target returns retryable `RunQueueConflict` before mutation.
    async fn prepare_git_root(
        &self,
        path: &Path,
        expected_root: Option<&Path>,
    ) -> Result<PathBuf, DomainError>;
    /// Verify an unchanged canonical exact Git root without initialization or repair.
    /// # Errors
    /// Stable Project path/init errors when the root is missing, rebound or nested.
    async fn verify_git_root(&self, expected_root: &Path) -> Result<(), DomainError>;
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
