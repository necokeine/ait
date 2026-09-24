//! Blocking local Git worktree boundary for the independent server.

use std::fmt::Debug;

/// A managed worktree returned by Git.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedWorktreeInfo {
    /// Absolute linked checkout root.
    pub path: String,
    /// Filesystem creation time in RFC 3339 form.
    pub created_at: String,
    /// Local branch, or none for detached HEAD.
    pub branch_name: Option<String>,
    /// Checked-out commit object name.
    pub head: Option<String>,
}

/// How a linked checkout obtains its branch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorktreeCreateMode {
    /// Create a new local branch from an optional base ref.
    BranchOff {
        /// Base ref; omission asks the adapter to resolve the repository default.
        base_ref: Option<String>,
        /// Desired local branch name.
        branch_name: String,
    },
    /// Check out an existing local or origin branch.
    Checkout {
        /// Existing branch name.
        branch_name: String,
    },
    /// Restore an archived worktree using its exact saved branch.
    Restore {
        /// Existing local branch; it must not be checked out elsewhere.
        branch_name: String,
        /// Saved comparison base, retained when it still resolves.
        base_ref: Option<String>,
    },
}

/// Input to the atomic Git portion of managed worktree creation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedWorktreeCreate {
    /// Source checkout directory, possibly below the repository root.
    pub cwd: String,
    /// Validated managed directory name.
    pub slug: String,
    /// Branch creation or checkout behavior.
    pub mode: WorktreeCreateMode,
}

/// Created Git placement before it is recorded in the workspace registry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreatedManagedWorktree {
    /// Main repository root.
    pub repo_root: String,
    /// Canonical source directory supplied by the caller.
    pub source_cwd: String,
    /// Source-relative directory in the linked checkout.
    pub workspace_cwd: String,
    /// Linked checkout root.
    pub worktree_path: String,
    /// Actual local branch, including collision suffixes.
    pub branch_name: String,
    /// Comparison base retained in the workspace registry.
    pub comparison_base_ref: Option<String>,
    /// Preferred origin URL, when configured.
    pub remote_url: Option<String>,
}

/// Ownership proof for an archive target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnedWorktree {
    /// Normalized managed worktree root, excluding a descendant workspace path.
    pub path: String,
    /// Main repository root when Git metadata is still available.
    pub repo_root: Option<String>,
}

/// Stable worktree adapter failure categories.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum WorktreeError {
    /// The selected path is outside a Git repository.
    #[error("Not in a git repository")]
    NotGitRepository,
    /// The target path is not inside the server-managed root.
    #[error("Worktree is not a Paseo-owned worktree")]
    NotAllowed,
    /// Checkout action omitted both a branch and change-request source.
    #[error("action \"checkout\" requires refName or checkoutSource")]
    MissingCheckoutTarget,
    /// An existing branch could not be resolved locally or from origin.
    #[error("Unknown branch: {0}")]
    UnknownBranch(String),
    /// A requested branch is already checked out and cannot be restored directly.
    #[error("Branch already checked out: {0}")]
    BranchAlreadyCheckedOut(String),
    /// A slug, branch, base ref, or path violates the worktree contract.
    #[error("{0}")]
    Invalid(String),
    /// A forge-backed checkout needs a service not installed in this phase.
    #[error("Change-request checkout requires a configured forge service")]
    ForgeUnavailable,
    /// Git or filesystem work failed.
    #[error("{0}")]
    Io(String),
}

/// Blocking adapter for server-owned linked Git checkouts.
pub trait ManagedWorktrees: Debug + Send + Sync {
    /// List managed worktrees belonging to the repository containing `cwd`.
    ///
    /// # Errors
    /// Returns a categorized Git or filesystem error.
    fn list(&self, cwd: &str) -> Result<Vec<ManagedWorktreeInfo>, WorktreeError>;

    /// Create one managed linked checkout and map the source-relative cwd into it.
    ///
    /// # Errors
    /// Returns a categorized validation, Git, or filesystem error. Failures after `git worktree
    /// add` make a best-effort rollback before returning.
    fn create(
        &self,
        input: &ManagedWorktreeCreate,
    ) -> Result<CreatedManagedWorktree, WorktreeError>;

    /// Resolve a path, possibly below a worktree root, into an ownership proof.
    ///
    /// # Errors
    /// Returns `NotAllowed` for paths outside the managed root.
    fn owned(&self, path: &str) -> Result<OwnedWorktree, WorktreeError>;

    /// Resolve a managed worktree path from repository and slug.
    ///
    /// # Errors
    /// Returns validation or Git inspection errors.
    fn path_for_slug(&self, repo_root: &str, slug: &str) -> Result<String, WorktreeError>;

    /// Return whether `candidate` is the root or a descendant of `root` after normalization.
    fn contains(&self, root: &str, candidate: &str) -> bool;

    /// Delete a proven managed worktree and prune stale Git administration state.
    ///
    /// # Errors
    /// Returns only after both Git and recursive filesystem removal fail to remove the target.
    fn remove(&self, worktree: &OwnedWorktree) -> Result<(), WorktreeError>;
}
