//! Blocking Git checkout read boundary.

/// Categorized checkout read failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckoutFailureKind {
    /// Directory is not a Git checkout.
    NotGitRepository,
    /// Input or path is not allowed.
    NotAllowed,
    /// Checkout contains unresolved conflicts.
    MergeConflict,
    /// Git, filesystem, timeout, or output failure.
    Unknown,
}

/// Checkout adapter failure with a local diagnostic.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct CheckoutRuntimeError {
    /// Stable error category.
    pub kind: CheckoutFailureKind,
    /// Human-readable local diagnostic.
    pub message: String,
}

/// Ahead/behind counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AheadBehind {
    /// Commits reachable only from HEAD.
    pub ahead: u64,
    /// Commits reachable only from the comparison ref.
    pub behind: u64,
}

/// Git/non-Git checkout status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckoutStatus {
    /// Whether Git metadata was found.
    pub is_git: bool,
    /// Worktree root.
    pub repo_root: Option<String>,
    /// Main repository root for linked worktrees.
    pub main_repo_root: Option<String>,
    /// Current branch.
    pub current_branch: Option<String>,
    /// Working tree dirtiness.
    pub is_dirty: Option<bool>,
    /// Display comparison base.
    pub base_ref: Option<String>,
    /// Counts against the comparison base.
    pub ahead_behind: Option<AheadBehind>,
    /// Exact configured upstream ref.
    pub upstream_ref: Option<String>,
    /// Commits ahead of upstream.
    pub ahead_of_origin: Option<u64>,
    /// Commits behind upstream.
    pub behind_of_origin: Option<u64>,
    /// Whether any remote exists.
    pub has_remote: bool,
    /// Preferred remote URL.
    pub remote_url: Option<String>,
    /// Whether the checkout is below the server-managed worktree root.
    pub is_managed_worktree: bool,
}

/// Diff comparison mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckoutDiffMode {
    /// Working tree and index against HEAD.
    Uncommitted,
    /// HEAD against a branch merge base.
    Base,
}

/// Diff comparison request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckoutDiffCompare {
    /// Comparison mode.
    pub mode: CheckoutDiffMode,
    /// Optional explicit base.
    pub base_ref: Option<String>,
    /// Ignore whitespace-only changes.
    pub ignore_whitespace: bool,
}

/// Structured line category.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffLineKind {
    /// Added line.
    Add,
    /// Removed line.
    Remove,
    /// Context line.
    Context,
    /// Hunk header.
    Header,
}

/// One structured diff line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffLine {
    /// Category.
    pub kind: DiffLineKind,
    /// Content without prefix.
    pub content: String,
}

/// One structured hunk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffHunk {
    /// First old line.
    pub old_start: u64,
    /// Old line count.
    pub old_count: u64,
    /// First new line.
    pub new_start: u64,
    /// New line count.
    pub new_count: u64,
    /// Header and body lines.
    pub lines: Vec<DiffLine>,
}

/// Structured diff status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParsedDiffStatus {
    /// Ordinary text.
    Ok,
    /// Budget placeholder.
    TooLarge,
    /// Binary placeholder.
    Binary,
}

/// One structured file diff.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedDiffFile {
    /// Destination path.
    pub path: String,
    /// Source path for a rename.
    pub old_path: Option<String>,
    /// New-file marker.
    pub is_new: bool,
    /// Deleted-file marker.
    pub is_deleted: bool,
    /// Added lines.
    pub additions: u64,
    /// Removed lines.
    pub deletions: u64,
    /// Parsed hunks.
    pub hunks: Vec<DiffHunk>,
    /// Optional placeholder status.
    pub status: Option<ParsedDiffStatus>,
}

/// Checkout diff snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckoutDiff {
    /// Path-sorted files.
    pub files: Vec<ParsedDiffFile>,
    /// Whether the aggregate diff exceeded its budget.
    pub diff_too_large: bool,
}

/// Commit file status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckoutCommitFileStatus {
    /// Added.
    Added,
    /// Modified/type changed.
    Modified,
    /// Deleted.
    Deleted,
    /// Renamed.
    Renamed,
}

/// File statistics attached to a commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckoutCommitFile {
    /// Destination path.
    pub path: String,
    /// Added lines.
    pub additions: u64,
    /// Removed lines.
    pub deletions: u64,
    /// Optional Git status.
    pub status: Option<CheckoutCommitFileStatus>,
}

/// One checkout commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckoutCommit {
    /// Full SHA.
    pub sha: String,
    /// Short SHA.
    pub short_sha: String,
    /// Subject.
    pub subject: String,
    /// Author name.
    pub author_name: String,
    /// ISO timestamp.
    pub author_date: String,
    /// Reachable from a remote ref.
    pub is_on_remote: bool,
    /// Belongs to bounded base context.
    pub is_on_base: bool,
    /// Changed files.
    pub files: Vec<CheckoutCommitFile>,
}

/// Commit history result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckoutCommits {
    /// Resolved comparison base.
    pub base_ref: Option<String>,
    /// Workspace commits followed by base context.
    pub commits: Vec<CheckoutCommit>,
}

/// Blocking read-only Git runtime.
pub trait CheckoutRuntime: std::fmt::Debug + Send + Sync {
    /// Inspect checkout status.
    ///
    /// # Errors
    /// Returns categorized path, Git, timeout, or output failures.
    fn status(&self, cwd: &str) -> Result<CheckoutStatus, CheckoutRuntimeError>;

    /// Force a complete checkout read. Stateless adapters may delegate to status.
    ///
    /// # Errors
    /// Returns categorized path, Git, timeout, or output failures.
    fn refresh(&self, cwd: &str) -> Result<(), CheckoutRuntimeError>;

    /// Produce a structured diff.
    ///
    /// # Errors
    /// Returns categorized input, Git, timeout, or output failures.
    fn diff(
        &self,
        cwd: &str,
        compare: &CheckoutDiffCompare,
    ) -> Result<CheckoutDiff, CheckoutRuntimeError>;

    /// List branch history and bounded base context.
    ///
    /// # Errors
    /// Returns categorized Git, timeout, or output failures.
    fn commits(&self, cwd: &str) -> Result<CheckoutCommits, CheckoutRuntimeError>;

    /// Read one textual file diff from a commit.
    ///
    /// # Errors
    /// Returns categorized input, Git, timeout, or output failures.
    fn commit_file_diff(
        &self,
        cwd: &str,
        sha: &str,
        path: &str,
    ) -> Result<Option<ParsedDiffFile>, CheckoutRuntimeError>;
}
