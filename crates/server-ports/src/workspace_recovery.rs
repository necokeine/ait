//! Filesystem and Git boundary for archived Workspace recovery.

use std::fmt::Debug;

/// Durable placement needed to recreate one archived managed worktree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchivedWorktreeRestore {
    /// Main repository that owns the linked checkout.
    pub source_repo_root: String,
    /// Exact deleted worktree root retained by the Workspace record.
    pub previous_worktree_root: String,
    /// Exact selected Workspace directory, possibly below the worktree root.
    pub workspace_cwd: String,
    /// Saved local branch to check out without inventing a replacement branch.
    pub branch: String,
    /// Saved comparison base, which may no longer resolve.
    pub base_ref: Option<String>,
}

/// Stable failures from local archived Workspace recovery.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum WorkspaceRecoveryRuntimeError {
    /// The saved branch is already checked out somewhere else.
    #[error("Branch already checked out: {0}")]
    BranchAlreadyCheckedOut(String),
    /// The saved branch cannot be resolved locally or from origin.
    #[error("Unknown branch: {0}")]
    UnknownBranch(String),
    /// Persisted placement cannot be restored safely.
    #[error("{0}")]
    Invalid(String),
    /// Git or filesystem work failed.
    #[error("{0}")]
    Io(String),
}

/// Blocking local runtime used by the Workspace recovery application service.
pub trait WorkspaceRecoveryRuntime: Debug + Send + Sync {
    /// Return true only when the exact path currently names a directory.
    fn is_directory(&self, path: &str) -> bool;

    /// Recreate the saved linked checkout and selected descendant directory.
    ///
    /// # Errors
    /// Returns a categorized validation, Git, or filesystem failure. An unsuccessful restore
    /// removes any worktree created during the attempt before returning where possible.
    fn restore_worktree(
        &self,
        input: &ArchivedWorktreeRestore,
    ) -> Result<(), WorkspaceRecoveryRuntimeError>;
}
