//! Project workspace capabilities consumed by the application layer.

mod commit;
pub use commit::{RunCommitBaseline, RunCommitPlan};

use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use ait_domain::DomainError;

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
/// Each public call shares one deadline across admission and nested phases, including
/// implicit lease acquisition. A timeout uses the operation's Project error code
/// and `details.reason = "timeout"`. If a business mutation may have started, errors
/// include `details.retained_paths` entries with `path` and `state`, repeat them in
/// the message for API callers, and disable blind retries (`retryable = false`).
/// A `*_started` state denotes an uncertain partial result, not confirmed success.
/// No method grants permission, moves a Session, publishes a Run, or writes storage.
#[async_trait::async_trait]
pub trait ProjectWorkspace: Send + Sync {
    /// Capture a clean baseline for optional Ait auto-commit.
    /// # Errors
    /// Dirty or unavailable repositories disable auto-commit, without blocking the Run.
    async fn capture_run_commit(&self, path: &Path) -> Result<RunCommitBaseline, DomainError> {
        let baseline = self.clean_baseline(path).await?;
        Ok(RunCommitBaseline {
            head: baseline.commit,
            index_tree: baseline.index_tree,
            branch: self.symbolic_head(path).await?,
        })
    }

    /// Prepare an exact commit object without moving HEAD or changing the user's index.
    /// `None` means the Run produced no committable changes.
    /// # Errors
    /// Rejects changed HEAD/index and unavailable Git. The caller persists the plan before publication.
    async fn prepare_run_commit(
        &self,
        _path: &Path,
        _baseline: &RunCommitBaseline,
        _run_id: &str,
    ) -> Result<Option<RunCommitPlan>, DomainError> {
        Err(DomainError::invariant(
            ait_domain::ErrorCode::InvalidConfiguration,
            "auto-commit unavailable",
        ))
    }

    /// Publish the exact prepared object once; retries reconcile the same commit identity.
    /// # Errors
    /// Rejects competing HEAD/index changes. Never reruns model work or overwrites working files.
    async fn publish_run_commit(
        &self,
        _path: &Path,
        _plan: &RunCommitPlan,
    ) -> Result<(), DomainError> {
        Err(DomainError::invariant(
            ait_domain::ErrorCode::InvalidConfiguration,
            "auto-commit unavailable",
        ))
    }
    /// Prepare and verify an exact canonical Git root; nested roots are allowed.
    ///
    /// If supplied, `expected_root` must still be the canonical target before mutation.
    ///
    /// # Errors
    ///
    /// Returns stable Project path/init errors. Non-UTF-8 paths fail closed, and a
    /// rebound expected target returns retryable `RunQueueConflict` before mutation.
    async fn prepare_git_root(
        &self,
        path: &Path,
        expected_root: Option<&Path>,
    ) -> Result<PathBuf, DomainError>;

    /// Verify an unchanged canonical exact Git root without initialization or repair.
    ///
    /// # Errors
    ///
    /// Returns stable Project path/init errors when the root is missing, rebound, or nested.
    async fn verify_git_root(&self, expected_root: &Path) -> Result<(), DomainError>;

    /// Read HEAD, creating only an empty initial commit for an unstaged unborn repository.
    ///
    /// # Errors
    ///
    /// Returns stable HEAD errors, including a staged unborn index.
    async fn ensure_git_head(&self, path: &Path) -> Result<String, DomainError>;

    /// Read a full HEAD object identity; `None` denotes an unborn HEAD.
    ///
    /// # Errors
    ///
    /// Returns stable HEAD errors for failed inspection or malformed output.
    async fn git_head(&self, path: &Path) -> Result<Option<String>, DomainError>;

    /// Compare HEAD and index before/after status, then require index to equal the HEAD tree.
    ///
    /// # Errors
    ///
    /// Returns dirty, unborn, moved HEAD/index, or unavailable Git errors.
    async fn clean_baseline(&self, path: &Path) -> Result<GitBaseline, DomainError>;

    /// Return the symbolic HEAD, or `None` for detached HEAD.
    ///
    /// # Errors
    ///
    /// Returns stable HEAD errors, including invalid Git output.
    async fn symbolic_head(&self, path: &Path) -> Result<Option<String>, DomainError>;

    /// Resolve the absolute Git metadata directory.
    ///
    /// # Errors
    ///
    /// Returns stable HEAD errors for unavailable or unresolvable Git metadata.
    async fn git_dir(&self, path: &Path) -> Result<PathBuf, DomainError>;

    /// Acquire canonical in-process admission and a nonblocking cross-process lock.
    ///
    /// # Errors
    ///
    /// Returns stable Project path or retryable workspace-busy failures.
    async fn acquire_lease(&self, path: &Path) -> Result<Arc<dyn WorkspaceLease>, DomainError>;

    /// Ensure a linked Session worktree, returning whether it was created.
    ///
    /// Only a newly created manager-owned worktree may be populated at `baseline`.
    /// An existing directory must be its own Git root and must never be reset.
    /// The supplied lease must protect `primary`; absent leases are acquired here.
    ///
    /// # Errors
    ///
    /// Returns stable path/Git/Session errors. Partial worktrees are retained on failure.
    async fn ensure_session_worktree(
        &self,
        primary: &Path,
        worktree: &Path,
        baseline: &str,
        lease: Option<Arc<dyn WorkspaceLease>>,
    ) -> Result<bool, DomainError>;

    /// Observe canonical root and nearest existing ancestor without granting access.
    ///
    /// # Errors
    ///
    /// Unresolvable paths, dangling symlinks, or non-UTF-8 paths fail closed.
    async fn path_facts(
        &self,
        root: &Path,
        destination: &Path,
    ) -> Result<WorkspacePathFacts, DomainError>;
}

/// Allocates a new workdir for a Project whose caller supplied only a name.
/// Platform default-directory resolution and filename rules belong to the adapter.
#[async_trait::async_trait]
pub trait ProjectDirectoryCreator: Send + Sync {
    /// Creates exactly one new directory and returns its absolute path.
    ///
    /// Existing entries, including files and symlinks, must fail without mutation.
    /// No parent directories may be implicitly created. Once returned, the path
    /// is retained even if Git preparation or registration subsequently fails.
    /// Async admission and filesystem phases consume one deadline. A timed-out
    /// creation returns [`ait_domain::ErrorCode::ProjectDirectoryCreationFailed`] with
    /// `reason = "timeout"`. If `mkdir` completed across that deadline, the error
    /// includes its retained path/state and is not automatically retryable.
    ///
    /// # Errors
    ///
    /// Returns a stable Project failure for invalid names, unavailable default
    /// directories, existing targets, or failed directory creation.
    async fn create_workdir(&self, name: &str) -> Result<PathBuf, DomainError>;
}

#[cfg(feature = "contract-tests")]
pub mod workspace_contract;
