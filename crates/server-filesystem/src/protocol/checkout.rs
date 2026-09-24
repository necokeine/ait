//! Git checkout status, diff, and commit-history payloads.

use serde::{Deserialize, Serialize};

/// Canonical checkout methods implemented by the independent server.
pub const CAPABILITIES: &[&str] = &[
    "checkout.status.get.request",
    "checkout.refresh.request",
    "checkout.diff.get.request",
    "checkout.diff.subscribe.request",
    "checkout.diff.unsubscribe.request",
    "checkout.commits.list.request",
    "checkout.commits.file_diff.request",
    "checkout.branch.validate.request",
    "checkout.branch.suggestions.request",
    "checkout.branch.switch.request",
    "checkout.rename_branch.request",
    "checkout.commit.request",
    "checkout.merge.request",
    "checkout.merge_from_base.request",
    "checkout.pull.request",
    "checkout.push.request",
    "checkout.discard_changes.request",
    "checkout.stash.save.request",
    "checkout.stash.pop.request",
    "checkout.stash.list.request",
];

/// A checkout-scoped request.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct CheckoutPathRequest {
    /// Directory inside the checkout.
    pub cwd: String,
}

/// Stable Paseo checkout error categories.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum CheckoutErrorCode {
    /// The directory is not in a Git repository.
    NotGitRepo,
    /// The requested read is outside the allowed checkout boundary.
    NotAllowed,
    /// The repository is in a conflicting state.
    MergeConflict,
    /// Another Git or filesystem failure occurred.
    Unknown,
}

/// Inline checkout error used by Paseo responses and updates.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CheckoutError {
    /// Stable machine-readable category.
    pub code: CheckoutErrorCode,
    /// Human-readable local diagnostic.
    pub message: String,
}

/// Ahead/behind counts for one comparison ref.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct AheadBehind {
    /// Commits reachable only from the checkout.
    pub ahead: u64,
    /// Commits reachable only from the comparison ref.
    pub behind: u64,
}

/// Checkout status response, including the non-Git null projection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckoutStatusResult {
    /// Echoed request directory.
    pub cwd: String,
    /// Whether Git metadata was found.
    pub is_git: bool,
    /// Checkout root, or null outside Git.
    pub repo_root: Option<String>,
    /// Main repository root for linked worktrees.
    pub main_repo_root: Option<String>,
    /// Current local branch, or null for detached HEAD/non-Git.
    pub current_branch: Option<String>,
    /// Working tree dirtiness, or null outside Git.
    pub is_dirty: Option<bool>,
    /// Comparison base display name.
    pub base_ref: Option<String>,
    /// Counts against the comparison base.
    pub ahead_behind: Option<AheadBehind>,
    /// Exact upstream ref resolved by Git.
    pub upstream_ref: Option<String>,
    /// Commits ahead of the exact upstream.
    pub ahead_of_origin: Option<u64>,
    /// Commits behind the exact upstream.
    pub behind_of_origin: Option<u64>,
    /// Whether any remote is configured.
    pub has_remote: bool,
    /// Preferred remote URL.
    pub remote_url: Option<String>,
    /// Whether the checkout is below the independent server's managed worktree root.
    pub is_paseo_owned_worktree: bool,
    /// Inline read error.
    pub error: Option<CheckoutError>,
}

impl Serialize for CheckoutStatusResult {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeMap;

        let mut map = serializer.serialize_map(Some(if self.is_git { 15 } else { 14 }))?;
        map.serialize_entry("cwd", &self.cwd)?;
        map.serialize_entry("isGit", &self.is_git)?;
        map.serialize_entry("repoRoot", &self.repo_root)?;
        if self.is_git {
            map.serialize_entry("mainRepoRoot", &self.main_repo_root)?;
        }
        map.serialize_entry("currentBranch", &self.current_branch)?;
        map.serialize_entry("isDirty", &self.is_dirty)?;
        map.serialize_entry("baseRef", &self.base_ref)?;
        map.serialize_entry("aheadBehind", &self.ahead_behind)?;
        map.serialize_entry("upstreamRef", &self.upstream_ref)?;
        map.serialize_entry("aheadOfOrigin", &self.ahead_of_origin)?;
        map.serialize_entry("behindOfOrigin", &self.behind_of_origin)?;
        map.serialize_entry("hasRemote", &self.has_remote)?;
        map.serialize_entry("remoteUrl", &self.remote_url)?;
        map.serialize_entry("isPaseoOwnedWorktree", &self.is_paseo_owned_worktree)?;
        map.serialize_entry("error", &self.error)?;
        map.end()
    }
}

/// Checkout refresh result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckoutRefreshResult {
    /// Echoed request directory.
    pub cwd: String,
    /// Whether the forced read completed.
    pub success: bool,
    /// Inline error.
    pub error: Option<CheckoutError>,
}

/// Diff comparison mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckoutDiffMode {
    /// Staged, unstaged, and untracked changes against HEAD.
    Uncommitted,
    /// Committed branch changes against a merge base.
    Base,
}

/// Diff comparison options.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckoutDiffCompare {
    /// Comparison mode.
    pub mode: CheckoutDiffMode,
    /// Explicit base ref for base mode.
    #[serde(default)]
    pub base_ref: Option<String>,
    /// Ignore whitespace-only changes.
    #[serde(default)]
    pub ignore_whitespace: bool,
}

/// One-shot checkout diff request.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct CheckoutDiffGetRequest {
    /// Directory inside the checkout.
    pub cwd: String,
    /// Comparison options.
    pub compare: CheckoutDiffCompare,
}

/// Connection-owned checkout diff subscription request.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckoutDiffSubscribeRequest {
    /// Optional caller-selected connection-local identity.
    #[serde(default)]
    pub subscription_id: Option<String>,
    /// Directory inside the checkout.
    pub cwd: String,
    /// Comparison options.
    pub compare: CheckoutDiffCompare,
}

/// Explicit checkout diff unsubscribe request.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckoutDiffUnsubscribeRequest {
    /// Connection-local subscription identity.
    pub subscription_id: String,
}

/// Syntax-highlight token. The independent server currently omits these optional values.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HighlightToken {
    /// Source text.
    pub text: String,
    /// Optional renderer class.
    pub style: Option<String>,
}

/// Unified diff line category.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiffLineKind {
    /// Added content.
    Add,
    /// Removed content.
    Remove,
    /// Unchanged context.
    Context,
    /// Hunk header.
    Header,
}

/// One structured diff line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DiffLine {
    /// Line category.
    #[serde(rename = "type")]
    pub kind: DiffLineKind,
    /// Content without the unified-diff prefix.
    pub content: String,
    /// Optional syntax-highlight tokens.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tokens: Option<Vec<HighlightToken>>,
}

/// One unified diff hunk.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiffHunk {
    /// First old-file line.
    pub old_start: u64,
    /// Old-file line count.
    pub old_count: u64,
    /// First new-file line.
    pub new_start: u64,
    /// New-file line count.
    pub new_count: u64,
    /// Header and body lines.
    pub lines: Vec<DiffLine>,
}

/// Structured diff placeholder/status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ParsedDiffStatus {
    /// Textual diff is present.
    Ok,
    /// The file exceeded a configured diff budget.
    TooLarge,
    /// The file is binary.
    Binary,
}

/// Structured file diff used by live diff and commit history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ParsedDiffFile {
    /// Destination path.
    pub path: String,
    /// Source path for a rename.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub old_path: Option<String>,
    /// Whether the destination is new.
    pub is_new: bool,
    /// Whether the file was deleted.
    pub is_deleted: bool,
    /// Added line count.
    pub additions: u64,
    /// Removed line count.
    pub deletions: u64,
    /// Parsed hunks.
    pub hunks: Vec<DiffHunk>,
    /// Optional status; omitted for ordinary Paseo-compatible text diffs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<ParsedDiffStatus>,
}

/// One-shot diff result and live-update body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckoutDiffResult {
    /// Directory from the subscription/request.
    pub cwd: String,
    /// Path-sorted file diffs.
    pub files: Vec<ParsedDiffFile>,
    /// Inline error.
    pub error: Option<CheckoutError>,
    /// True when the total diff was not safe to return.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diff_too_large: Option<bool>,
}

/// Initial diff subscription result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckoutDiffSubscriptionResult {
    /// Connection-local subscription identity.
    pub subscription_id: String,
    /// Directory from the request.
    pub cwd: String,
    /// Initial files.
    pub files: Vec<ParsedDiffFile>,
    /// Inline error.
    pub error: Option<CheckoutError>,
    /// True when the total diff was not safe to return.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diff_too_large: Option<bool>,
}

/// Git commit file status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckoutCommitFileStatus {
    /// Added file.
    Added,
    /// Modified or type-changed file.
    Modified,
    /// Deleted file.
    Deleted,
    /// Renamed file.
    Renamed,
}

/// Per-file commit statistics.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CheckoutCommitFile {
    /// Destination path.
    pub path: String,
    /// Added lines; binary files use zero.
    pub additions: u64,
    /// Removed lines; binary files use zero.
    pub deletions: u64,
    /// Optional status when Git supplies one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<CheckoutCommitFileStatus>,
}

/// One checkout history commit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckoutCommit {
    /// Full object identity.
    pub sha: String,
    /// Abbreviated object identity.
    pub short_sha: String,
    /// First-line subject.
    pub subject: String,
    /// Git author display name.
    pub author_name: String,
    /// ISO 8601 author timestamp.
    pub author_date: String,
    /// False when reachable from no remote ref.
    pub is_on_remote: bool,
    /// True for bounded base-history context.
    pub is_on_base: bool,
    /// Changed files.
    pub files: Vec<CheckoutCommitFile>,
}

/// Checkout commit-list result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckoutCommitsListResult {
    /// Echoed request directory.
    pub cwd: String,
    /// Resolved comparison ref.
    pub base_ref: Option<String>,
    /// Workspace commits followed by up to ten base-context commits.
    pub commits: Vec<CheckoutCommit>,
    /// Inline error.
    pub error: Option<CheckoutError>,
}

/// Single-file commit diff request.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct CheckoutCommitFileDiffRequest {
    /// Directory inside the checkout.
    pub cwd: String,
    /// Commit-ish to inspect.
    pub sha: String,
    /// Safe repository-relative path.
    pub path: String,
}

/// Single-file commit diff result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CheckoutCommitFileDiffResult {
    /// Echoed request directory.
    pub cwd: String,
    /// Echoed commit-ish.
    pub sha: String,
    /// Echoed relative path.
    pub path: String,
    /// Textual diff, or null for missing/binary content.
    pub file: Option<ParsedDiffFile>,
    /// Inline error.
    pub error: Option<CheckoutError>,
}

/// Branch existence request.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckoutBranchValidateRequest {
    /// Directory inside the checkout.
    pub cwd: String,
    /// Local name or origin-qualified name to resolve.
    pub branch_name: String,
}

/// Branch existence result. Paseo uses a plain string error for this query.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckoutBranchValidateResult {
    /// Whether the branch exists locally or on origin.
    pub exists: bool,
    /// Normalized local branch name.
    pub resolved_ref: Option<String>,
    /// Whether only the origin tracking ref exists.
    pub is_remote: bool,
    /// Inline validation or Git error.
    pub error: Option<String>,
}

/// Branch suggestion request.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct CheckoutBranchSuggestionsRequest {
    /// Directory inside the checkout.
    pub cwd: String,
    /// Optional case-insensitive substring query.
    #[serde(default)]
    pub query: Option<String>,
    /// Optional result limit in the inclusive range 1..=200.
    #[serde(default)]
    pub limit: Option<usize>,
}

/// One branch suggestion and its local/origin state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckoutBranchSuggestion {
    /// Normalized local branch name.
    pub name: String,
    /// Committer timestamp in Unix seconds.
    pub committer_date: i64,
    /// Whether a local branch exists.
    pub has_local: bool,
    /// Whether an origin tracking ref exists.
    pub has_remote: bool,
    /// Commits present only on the local branch.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub local_ahead: Option<u64>,
    /// Commits present only on the origin tracking ref.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub local_behind: Option<u64>,
}

/// Branch suggestions result. Paseo uses a plain string error for this query.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckoutBranchSuggestionsResult {
    /// Ordered branch names retained for compatibility.
    pub branches: Vec<String>,
    /// Ordered branch details.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch_details: Option<Vec<CheckoutBranchSuggestion>>,
    /// Inline Git error.
    pub error: Option<String>,
}

/// Existing-branch checkout request.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct CheckoutBranchSwitchRequest {
    /// Directory inside the checkout.
    pub cwd: String,
    /// Existing local or origin branch.
    pub branch: String,
}

/// Existing-branch checkout source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CheckoutBranchSource {
    /// Existing local branch.
    Local,
    /// Origin-only branch materialized locally.
    Remote,
}

/// Existing-branch checkout result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckoutBranchSwitchResult {
    /// Echoed request directory.
    pub cwd: String,
    /// Whether checkout completed.
    pub success: bool,
    /// Echoed requested branch.
    pub branch: String,
    /// Resolution source on success.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<CheckoutBranchSource>,
    /// Inline checkout error.
    pub error: Option<CheckoutError>,
}

/// Current-branch rename request.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct CheckoutBranchRenameRequest {
    /// Directory inside the checkout.
    pub cwd: String,
    /// New local branch name.
    pub branch: String,
}

/// Current-branch rename result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckoutBranchRenameResult {
    /// Whether rename completed.
    pub success: bool,
    /// Echoed request directory.
    pub cwd: String,
    /// Renamed branch on success.
    pub current_branch: Option<String>,
    /// Inline checkout error.
    pub error: Option<CheckoutError>,
}

/// Commit request.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckoutCommitRequest {
    /// Directory inside the checkout.
    pub cwd: String,
    /// Explicit commit message.
    #[serde(default)]
    pub message: Option<String>,
    /// Whether all changes should be staged first. Defaults to true.
    #[serde(default)]
    pub add_all: Option<bool>,
}

/// Merge strategy for merging the current branch into its base.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CheckoutMergeStrategy {
    /// Ordinary Git merge.
    Merge,
    /// Squash and commit the resulting tree.
    Squash,
}

/// Merge-current-branch-to-base request.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckoutMergeRequest {
    /// Directory inside the current feature checkout.
    pub cwd: String,
    /// Optional base branch override.
    #[serde(default)]
    pub base_ref: Option<String>,
    /// Merge strategy. Defaults to merge.
    #[serde(default)]
    pub strategy: Option<CheckoutMergeStrategy>,
    /// Require the request checkout to be clean before operating.
    #[serde(default)]
    pub require_clean_target: Option<bool>,
}

/// Merge-base-into-current-branch request.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckoutMergeFromBaseRequest {
    /// Directory inside the current feature checkout.
    pub cwd: String,
    /// Optional base branch override.
    #[serde(default)]
    pub base_ref: Option<String>,
    /// Require a clean current checkout. Defaults to true.
    #[serde(default)]
    pub require_clean_target: Option<bool>,
}

/// Path-scoped discard request.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct CheckoutDiscardChangesRequest {
    /// Directory inside the checkout.
    pub cwd: String,
    /// Literal repository-relative paths to restore/remove.
    pub paths: Vec<String>,
}

/// Paseo stash save request.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct CheckoutStashSaveRequest {
    /// Directory inside the checkout.
    pub cwd: String,
    /// Optional branch label embedded in the stash message.
    #[serde(default)]
    pub branch: Option<String>,
}

/// Paseo stash pop request.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckoutStashPopRequest {
    /// Directory inside the checkout.
    pub cwd: String,
    /// Zero-based stash index.
    pub stash_index: usize,
}

/// Paseo stash list request.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckoutStashListRequest {
    /// Directory inside the checkout.
    pub cwd: String,
    /// Return only Paseo-created stashes. Defaults to true.
    #[serde(default)]
    pub paseo_only: Option<bool>,
}

/// One stash entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckoutStashEntry {
    /// Zero-based stash index.
    pub index: usize,
    /// Full Git stash subject.
    pub message: String,
    /// Paseo branch label, when present.
    pub branch: Option<String>,
    /// Whether the stash uses the Paseo prefix.
    pub is_paseo: bool,
}

/// Stash list result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CheckoutStashListResult {
    /// Echoed request directory.
    pub cwd: String,
    /// Filtered stash entries.
    pub entries: Vec<CheckoutStashEntry>,
    /// Inline checkout error.
    pub error: Option<CheckoutError>,
}

/// Shared result shape for checkout mutations without extra result fields.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CheckoutMutationResult {
    /// Echoed request directory.
    pub cwd: String,
    /// Whether the mutation completed.
    pub success: bool,
    /// Inline checkout error.
    pub error: Option<CheckoutError>,
}

#[cfg(test)]
mod tests;
