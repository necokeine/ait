//! Blocking forge adapter contract.

use serde_json::Value;

/// Forge failure category.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForgeFailureKind {
    /// Directory is outside a Git repository.
    NotGitRepository,
    /// Input or path is not allowed.
    NotAllowed,
    /// Pull request has merge conflicts.
    MergeConflict,
    /// Required forge CLI is absent.
    CliMissing,
    /// Forge CLI is not authenticated.
    Unauthenticated,
    /// No supported forge remote is configured.
    NoRemote,
    /// Forge object was not found.
    NotFound,
    /// Forge denied access.
    Forbidden,
    /// Request parameters are invalid.
    Invalid,
    /// Command, network, parsing, or another forge failure.
    Unknown,
}

/// Forge adapter failure with a local diagnostic.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct ForgeRuntimeError {
    /// Stable failure category.
    pub kind: ForgeFailureKind,
    /// Human-readable diagnostic.
    pub message: String,
}

/// Forge availability state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForgeAuthState {
    /// CLI is available and authenticated.
    Authenticated,
    /// CLI has no usable credentials.
    Unauthenticated,
    /// CLI executable is absent.
    CliMissing,
    /// Checkout has no supported forge remote.
    NoRemote,
    /// A non-authentication forge read failed.
    Error,
}

/// Normalized forge search category.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForgeSearchKind {
    /// Issue.
    Issue,
    /// Pull or merge request.
    ChangeRequest,
}

/// One forge search item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForgeSearchItem {
    /// Result category.
    pub kind: ForgeSearchKind,
    /// Forge brand when resolved.
    pub forge: Option<String>,
    /// Forge-local number.
    pub number: u64,
    /// Title.
    pub title: String,
    /// Browser URL.
    pub url: String,
    /// Open forge state.
    pub state: String,
    /// Body.
    pub body: Option<String>,
    /// Label names.
    pub labels: Vec<String>,
    /// Full project path.
    pub project_path: Option<String>,
    /// Base branch for change requests.
    pub base_ref_name: Option<String>,
    /// Head branch for change requests.
    pub head_ref_name: Option<String>,
    /// Forge timestamp.
    pub updated_at: Option<String>,
}

/// Forge search outcome, including unavailable results that are not request failures.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForgeSearch {
    /// Search results.
    pub items: Vec<ForgeSearchItem>,
    /// Availability state.
    pub auth_state: ForgeAuthState,
}

/// Pull request merge method.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PullRequestMergeMethod {
    /// Merge commit.
    Merge,
    /// Squash merge.
    Squash,
    /// Rebase merge.
    Rebase,
}

/// Created pull request identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PullRequestCreated {
    /// Browser URL.
    pub url: String,
    /// Pull request number.
    pub number: u64,
}

/// Pull request mergeability.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PullRequestMergeable {
    /// Mergeable.
    Mergeable,
    /// Conflicting.
    Conflicting,
    /// Unknown.
    Unknown,
}

/// One normalized pull request check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PullRequestCheck {
    /// Check name.
    pub name: String,
    /// Normalized lifecycle.
    pub status: String,
    /// Details URL.
    pub url: Option<String>,
    /// Workflow display name.
    pub workflow: Option<String>,
    /// Formatted duration.
    pub duration: Option<String>,
    /// Check-run identifier.
    pub check_run_id: Option<u64>,
    /// Workflow-run identifier.
    pub workflow_run_id: Option<u64>,
    /// Open refinements.
    pub traits: Option<Vec<String>>,
}

/// Current pull request status.
#[derive(Debug, Clone, PartialEq)]
pub struct PullRequestStatus {
    /// Forge brand.
    pub forge: String,
    /// Full project path.
    pub project_path: Option<String>,
    /// Pull request number.
    pub number: Option<u64>,
    /// Browser URL.
    pub url: String,
    /// Title.
    pub title: String,
    /// Forge state.
    pub state: String,
    /// Base branch.
    pub base_ref_name: String,
    /// Head branch.
    pub head_ref_name: String,
    /// Merged state.
    pub is_merged: bool,
    /// Draft state.
    pub is_draft: bool,
    /// Mergeability.
    pub mergeable: PullRequestMergeable,
    /// Checks.
    pub checks: Vec<PullRequestCheck>,
    /// Aggregate check state.
    pub checks_status: String,
    /// Review decision.
    pub review_decision: Option<String>,
    /// Repository owner.
    pub repo_owner: Option<String>,
    /// Repository name.
    pub repo_name: Option<String>,
    /// Legacy GitHub facts mirror.
    pub github: Option<Value>,
    /// Forge-specific facts.
    pub forge_specific: Option<Value>,
}

/// Current pull request read, including forge availability.
#[derive(Debug, Clone, PartialEq)]
pub struct PullRequestStatusRead {
    /// Current pull request, or none.
    pub status: Option<PullRequestStatus>,
    /// Availability state.
    pub auth_state: ForgeAuthState,
    /// Resolved forge brand.
    pub forge: Option<String>,
}

/// Timeline review state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimelineReviewState {
    /// Approved review.
    Approved,
    /// Changes requested.
    ChangesRequested,
    /// General review comment.
    Commented,
}

/// Optional inline comment location.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimelineCommentLocation {
    /// Repository-relative path.
    pub path: String,
    /// Ending line.
    pub line: Option<u64>,
    /// Starting line.
    pub start_line: Option<u64>,
    /// Thread identifier.
    pub thread_id: Option<String>,
    /// Resolution state.
    pub is_resolved: Option<bool>,
    /// Outdated state.
    pub is_outdated: Option<bool>,
}

/// Pull request review or comment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PullRequestTimelineItem {
    /// Review.
    Review {
        /// Node identifier.
        id: String,
        /// Author login.
        author: String,
        /// Author profile URL.
        author_url: Option<String>,
        /// Author avatar URL.
        avatar_url: Option<String>,
        /// Markdown body.
        body: String,
        /// Unix epoch milliseconds.
        created_at: i64,
        /// Browser URL.
        url: String,
        /// Review decision.
        review_state: TimelineReviewState,
    },
    /// Comment.
    Comment {
        /// Node identifier.
        id: String,
        /// Author login.
        author: String,
        /// Author profile URL.
        author_url: Option<String>,
        /// Author avatar URL.
        avatar_url: Option<String>,
        /// Markdown body.
        body: String,
        /// Unix epoch milliseconds.
        created_at: i64,
        /// Browser URL.
        url: String,
        /// Parent review identifier.
        review_id: Option<String>,
        /// General thread identifier.
        thread_id: Option<String>,
        /// General thread resolution state.
        thread_is_resolved: Option<bool>,
        /// Inline location.
        location: Option<TimelineCommentLocation>,
    },
}

/// Timeline error category.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimelineErrorKind {
    /// Pull request not found.
    NotFound,
    /// Access forbidden.
    Forbidden,
    /// Another failure.
    Unknown,
}

/// Inline timeline error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimelineError {
    /// Stable category.
    pub kind: TimelineErrorKind,
    /// Local diagnostic.
    pub message: String,
}

/// Pull request timeline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PullRequestTimeline {
    /// Pull request number.
    pub pr_number: u64,
    /// Stable items.
    pub items: Vec<PullRequestTimelineItem>,
    /// Page truncation.
    pub truncated: bool,
    /// Inline forge error.
    pub error: Option<TimelineError>,
    /// Forge availability.
    pub auth_state: ForgeAuthState,
}

/// Check annotation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckAnnotation {
    /// Repository-relative path.
    pub path: Option<String>,
    /// First line.
    pub start_line: Option<u64>,
    /// Last line.
    pub end_line: Option<u64>,
    /// Severity.
    pub annotation_level: Option<String>,
    /// Message.
    pub message: Option<String>,
    /// Title.
    pub title: Option<String>,
    /// Additional details.
    pub raw_details: Option<String>,
}

/// Failed workflow job.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckFailedJob {
    /// Job identifier.
    pub job_id: u64,
    /// Job name.
    pub name: String,
    /// Forge state.
    pub status: Option<String>,
    /// Forge conclusion.
    pub conclusion: Option<String>,
    /// Browser URL.
    pub url: Option<String>,
    /// Bounded log tail.
    pub log_tail: Option<String>,
    /// Log truncation.
    pub log_truncated: Option<bool>,
}

/// Check output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckOutput {
    /// Title.
    pub title: Option<String>,
    /// Markdown summary.
    pub summary: Option<String>,
    /// Markdown detail.
    pub text: Option<String>,
}

/// Detailed check result.
#[derive(Debug, Clone, PartialEq)]
pub struct CheckDetails {
    /// Check-run identifier.
    pub check_run_id: u64,
    /// Workflow-run identifier.
    pub workflow_run_id: Option<u64>,
    /// Check name.
    pub name: String,
    /// Forge state.
    pub status: Option<String>,
    /// Forge conclusion.
    pub conclusion: Option<String>,
    /// Browser URL.
    pub url: Option<String>,
    /// Details URL.
    pub details_url: Option<String>,
    /// Output.
    pub output: Option<CheckOutput>,
    /// Annotations.
    pub annotations: Vec<CheckAnnotation>,
    /// Failed jobs.
    pub failed_jobs: Vec<CheckFailedJob>,
    /// Truncation flag.
    pub truncated: bool,
    /// Pipeline-oriented forge details.
    pub pipeline: Option<Value>,
}

/// Blocking forge runtime.
pub trait ForgeRuntime: std::fmt::Debug + Send {
    /// Search issues and change requests.
    ///
    /// # Errors
    /// Returns categorized local Git, CLI, authentication, or forge failures.
    fn search(
        &self,
        cwd: &str,
        query: &str,
        limit: usize,
        kinds: &[ForgeSearchKind],
    ) -> Result<ForgeSearch, ForgeRuntimeError>;

    /// Push the current branch and create a pull request.
    ///
    /// # Errors
    /// Returns categorized local Git, push, CLI, authentication, or forge failures.
    fn create_pull_request(
        &self,
        cwd: &str,
        title: &str,
        body: &str,
        base_ref: Option<&str>,
    ) -> Result<PullRequestCreated, ForgeRuntimeError>;

    /// Read the current branch's pull request.
    ///
    /// # Errors
    /// Returns categorized local Git, CLI, authentication, or forge failures.
    fn current_pull_request_status(
        &self,
        cwd: &str,
    ) -> Result<PullRequestStatusRead, ForgeRuntimeError>;

    /// Merge the current pull request.
    ///
    /// # Errors
    /// Returns resolution, validation, CLI, authentication, or forge failures.
    fn merge_current_pull_request(
        &self,
        cwd: &str,
        merge_method: PullRequestMergeMethod,
    ) -> Result<(), ForgeRuntimeError>;

    /// Enable or disable auto-merge on the current pull request.
    ///
    /// # Errors
    /// Returns resolution, validation, CLI, authentication, or forge failures.
    fn set_current_pull_request_auto_merge(
        &self,
        cwd: &str,
        enabled: bool,
        merge_method: Option<PullRequestMergeMethod>,
    ) -> Result<(), ForgeRuntimeError>;

    /// Read a pull request timeline.
    ///
    /// # Errors
    /// Returns identity, CLI, authentication, or forge failures.
    fn pull_request_timeline(
        &self,
        cwd: &str,
        pr_number: u64,
        repo_owner: &str,
        repo_name: &str,
    ) -> Result<PullRequestTimeline, ForgeRuntimeError>;

    /// Read detailed CI check data.
    ///
    /// # Errors
    /// Returns check identity, CLI, authentication, or forge failures.
    fn check_details(
        &self,
        cwd: &str,
        repo_owner: Option<&str>,
        repo_name: Option<&str>,
        check_run_id: Option<u64>,
        workflow_run_id: Option<u64>,
        change_request_number: Option<u64>,
    ) -> Result<CheckDetails, ForgeRuntimeError>;
}
