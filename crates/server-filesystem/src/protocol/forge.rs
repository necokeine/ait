//! Forge search, pull request, timeline, and check-detail payloads.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Canonical forge methods implemented by the independent server.
pub const CAPABILITIES: &[&str] = &[
    "forge.search.request",
    "github.search.request",
    "checkout.pr.create.request",
    "checkout.pr.merge.request",
    "checkout.pr.status.request",
    "checkout.pr.timeline.request",
    "checkout.forge.set_auto_merge.request",
    "checkout.forge.get_check_details.request",
    "checkout.github.set_auto_merge.request",
    "checkout.github.get_check_details.request",
];

/// Stable forge availability state copied from Paseo.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ForgeAuthState {
    /// The forge CLI is installed and authenticated.
    Authenticated,
    /// The CLI is installed but has no usable credentials.
    Unauthenticated,
    /// The forge CLI is absent.
    CliMissing,
    /// The checkout has no supported forge remote.
    NoRemote,
    /// A non-authentication forge read failed.
    Error,
}

/// Search categories, including Paseo's temporary GitHub aliases.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum ForgeSearchKind {
    /// Forge-neutral issue.
    #[serde(rename = "issue")]
    Issue,
    /// Forge-neutral pull or merge request.
    #[serde(rename = "change_request")]
    ChangeRequest,
    /// Legacy GitHub issue spelling.
    #[serde(rename = "github-issue")]
    GithubIssue,
    /// Legacy GitHub pull request spelling.
    #[serde(rename = "github-pr")]
    GithubPr,
    /// Legacy short pull request spelling.
    #[serde(rename = "pr")]
    Pr,
}

/// Forge or compatibility GitHub search request.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ForgeSearchRequest {
    /// Checkout used to resolve the forge and repository.
    pub cwd: String,
    /// Forge search query.
    pub query: String,
    /// Optional result limit, from one through fifty.
    pub limit: Option<usize>,
    /// Optional result categories.
    pub kinds: Option<Vec<ForgeSearchKind>>,
}

/// One forge search result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ForgeSearchItem {
    /// `issue`, `change_request`, or the legacy `pr` projection.
    pub kind: String,
    /// Resolved forge brand.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub forge: Option<String>,
    /// Forge-local issue or change-request number.
    pub number: u64,
    /// Display title.
    pub title: String,
    /// Browser URL.
    pub url: String,
    /// Open forge state.
    pub state: String,
    /// Optional body.
    pub body: Option<String>,
    /// Label names.
    pub labels: Vec<String>,
    /// Full repository path when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_path: Option<String>,
    /// Base branch for change requests.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_ref_name: Option<String>,
    /// Head branch for change requests.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub head_ref_name: Option<String>,
    /// Forge timestamp.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
}

/// Neutral forge search response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ForgeSearchResult {
    /// Matching issues and change requests.
    pub items: Vec<ForgeSearchItem>,
    /// Forge availability when it is known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth_state: Option<ForgeAuthState>,
    /// Non-authentication failure text.
    pub error: Option<String>,
}

/// GitHub compatibility search response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GithubSearchResult {
    /// Matching issues and pull requests.
    pub items: Vec<ForgeSearchItem>,
    /// Legacy availability flag.
    pub features_enabled: bool,
    /// Forge availability.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth_state: Option<ForgeAuthState>,
    /// Older legacy availability flag.
    pub github_features_enabled: bool,
    /// Non-authentication failure text.
    pub error: Option<String>,
}

/// Create a pull request for the current branch.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestCreateRequest {
    /// Checkout directory.
    pub cwd: String,
    /// Optional explicit title.
    pub title: Option<String>,
    /// Optional explicit body.
    pub body: Option<String>,
    /// Optional base branch or ref.
    pub base_ref: Option<String>,
}

/// Pull request merge method.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PullRequestMergeMethod {
    /// Create a merge commit.
    Merge,
    /// Squash the pull request.
    Squash,
    /// Rebase the pull request.
    Rebase,
}

/// Merge the pull request associated with the current branch.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestMergeRequest {
    /// Checkout directory.
    pub cwd: String,
    /// Requested forge merge method.
    pub merge_method: PullRequestMergeMethod,
}

/// Enable or disable auto-merge for the current pull request.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestAutoMergeRequest {
    /// Checkout directory.
    pub cwd: String,
    /// Desired auto-merge state.
    pub enabled: bool,
    /// Required when enabling and forbidden when disabling.
    pub merge_method: Option<PullRequestMergeMethod>,
}

/// A checkout-scoped forge read request.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ForgePathRequest {
    /// Checkout directory.
    pub cwd: String,
}

/// Pull request timeline request.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestTimelineRequest {
    /// Checkout directory.
    pub cwd: String,
    /// Pull request number.
    pub pr_number: u64,
    /// GitHub repository owner.
    pub repo_owner: String,
    /// GitHub repository name.
    pub repo_name: String,
}

/// Detailed check request.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckDetailsRequest {
    /// Checkout directory.
    pub cwd: String,
    /// GitHub repository owner.
    pub repo_owner: Option<String>,
    /// GitHub repository name.
    pub repo_name: Option<String>,
    /// GitHub check-run identifier.
    pub check_run_id: Option<u64>,
    /// GitHub Actions workflow-run identifier.
    pub workflow_run_id: Option<u64>,
    /// Change request number used by other forge families.
    pub change_request_number: Option<u64>,
}

/// Pull request creation response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PullRequestCreateResult {
    /// Echoed checkout directory.
    pub cwd: String,
    /// Browser URL on success.
    pub url: Option<String>,
    /// Pull request number on success.
    pub number: Option<u64>,
    /// Inline checkout-shaped error.
    pub error: Option<crate::protocol::checkout::CheckoutError>,
}

/// Generic pull request mutation response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PullRequestMutationResult {
    /// Echoed checkout directory.
    pub cwd: String,
    /// Whether the operation completed.
    pub success: bool,
    /// Inline checkout-shaped error.
    pub error: Option<crate::protocol::checkout::CheckoutError>,
}

/// Auto-merge mutation response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PullRequestAutoMergeResult {
    /// Echoed checkout directory.
    pub cwd: String,
    /// Requested state.
    pub enabled: bool,
    /// Whether the operation completed.
    pub success: bool,
    /// Inline checkout-shaped error.
    pub error: Option<crate::protocol::checkout::CheckoutError>,
}

/// Pull request mergeability.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum PullRequestMergeable {
    /// Forge reports the request can merge.
    Mergeable,
    /// Forge reports conflicts.
    Conflicting,
    /// Forge does not know yet.
    Unknown,
}

/// One normalized CI check.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestCheck {
    /// Check name.
    pub name: String,
    /// Normalized lifecycle.
    pub status: String,
    /// Details URL.
    pub url: Option<String>,
    /// Workflow display name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workflow: Option<String>,
    /// Formatted run duration.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration: Option<String>,
    /// Check-run identifier.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub check_run_id: Option<u64>,
    /// Workflow-run identifier.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workflow_run_id: Option<u64>,
    /// Open forge-neutral refinements.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub traits: Option<Vec<String>>,
}

/// Current pull request status.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestStatus {
    /// Resolved forge brand.
    pub forge: String,
    /// Full forge project path.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_path: Option<String>,
    /// Pull request number.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub number: Option<u64>,
    /// Browser URL.
    pub url: String,
    /// Display title.
    pub title: String,
    /// Forge state.
    pub state: String,
    /// Base branch.
    pub base_ref_name: String,
    /// Head branch.
    pub head_ref_name: String,
    /// Whether the request is merged.
    pub is_merged: bool,
    /// Whether it is a draft.
    pub is_draft: bool,
    /// Forge mergeability.
    pub mergeable: PullRequestMergeable,
    /// Normalized checks.
    pub checks: Vec<PullRequestCheck>,
    /// Aggregate check state.
    pub checks_status: String,
    /// Review decision.
    pub review_decision: Option<String>,
    /// Repository owner.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repo_owner: Option<String>,
    /// Repository name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repo_name: Option<String>,
    /// Legacy GitHub facts mirror.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub github: Option<Value>,
    /// Open forge-specific facts envelope.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub forge_specific: Option<Value>,
}

/// Current pull request status response.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestStatusResult {
    /// Echoed checkout directory.
    pub cwd: String,
    /// Current pull request, or null.
    pub status: Option<PullRequestStatus>,
    /// Paseo compatibility availability flag.
    pub github_features_enabled: bool,
    /// Forge availability.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth_state: Option<ForgeAuthState>,
    /// Resolved forge brand.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub forge: Option<String>,
    /// Inline checkout-shaped error.
    pub error: Option<crate::protocol::checkout::CheckoutError>,
}

/// Pull request timeline review state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TimelineReviewState {
    /// Approved review.
    Approved,
    /// Changes requested.
    ChangesRequested,
    /// General review comment.
    Commented,
}

/// Optional file position for a timeline comment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TimelineCommentLocation {
    /// Repository-relative path.
    pub path: String,
    /// Ending line.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<u64>,
    /// Starting line.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_line: Option<u64>,
    /// Forge thread identifier.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
    /// Resolution state.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_resolved: Option<bool>,
    /// Whether the location is stale.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_outdated: Option<bool>,
}

/// Timeline review or comment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PullRequestTimelineItem {
    /// Pull request review.
    Review {
        /// Forge node identifier.
        id: String,
        /// Author login.
        author: String,
        /// Author profile URL.
        #[serde(rename = "authorUrl")]
        author_url: Option<String>,
        /// Author avatar URL.
        #[serde(rename = "avatarUrl")]
        avatar_url: Option<String>,
        /// Markdown body.
        body: String,
        /// Unix epoch milliseconds.
        #[serde(rename = "createdAt")]
        created_at: i64,
        /// Browser URL.
        url: String,
        /// Normalized review decision.
        #[serde(rename = "reviewState")]
        review_state: TimelineReviewState,
    },
    /// General or inline comment.
    Comment {
        /// Forge node identifier.
        id: String,
        /// Author login.
        author: String,
        /// Author profile URL.
        #[serde(rename = "authorUrl")]
        author_url: Option<String>,
        /// Author avatar URL.
        #[serde(rename = "avatarUrl")]
        avatar_url: Option<String>,
        /// Markdown body.
        body: String,
        /// Unix epoch milliseconds.
        #[serde(rename = "createdAt")]
        created_at: i64,
        /// Browser URL.
        url: String,
        /// Parent review identifier.
        #[serde(rename = "reviewId", skip_serializing_if = "Option::is_none")]
        review_id: Option<String>,
        /// Thread identifier independent of a file location.
        #[serde(rename = "threadId", skip_serializing_if = "Option::is_none")]
        thread_id: Option<String>,
        /// General-thread resolution state.
        #[serde(rename = "threadIsResolved", skip_serializing_if = "Option::is_none")]
        thread_is_resolved: Option<bool>,
        /// Optional inline position.
        #[serde(skip_serializing_if = "Option::is_none")]
        location: Option<TimelineCommentLocation>,
    },
}

/// Pull request timeline error category.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TimelineErrorKind {
    /// Pull request not found.
    NotFound,
    /// Caller lacks permission.
    Forbidden,
    /// Another failure occurred.
    Unknown,
}

/// Inline timeline error.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TimelineError {
    /// Stable category.
    pub kind: TimelineErrorKind,
    /// Local diagnostic.
    pub message: String,
}

/// Pull request timeline response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestTimelineResult {
    /// Echoed checkout directory.
    pub cwd: String,
    /// Pull request number.
    pub pr_number: Option<u64>,
    /// Stable timeline items.
    pub items: Vec<PullRequestTimelineItem>,
    /// Whether a forge page limit truncated the timeline.
    pub truncated: bool,
    /// Inline timeline error.
    pub error: Option<TimelineError>,
    /// Paseo compatibility availability flag.
    pub github_features_enabled: bool,
    /// Forge availability.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth_state: Option<ForgeAuthState>,
}

/// Check-run annotation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckAnnotation {
    /// Repository-relative path.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// First annotated line.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_line: Option<u64>,
    /// Last annotated line.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_line: Option<u64>,
    /// Forge annotation severity.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub annotation_level: Option<String>,
    /// Main message.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// Annotation title.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Additional details.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw_details: Option<String>,
}

/// Failed workflow job summary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckFailedJob {
    /// Job identifier.
    pub job_id: u64,
    /// Job name.
    pub name: String,
    /// Forge job state.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// Forge conclusion.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conclusion: Option<String>,
    /// Browser URL.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Bounded log tail.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub log_tail: Option<String>,
    /// Whether the log was truncated.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub log_truncated: Option<bool>,
}

/// GitHub check output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CheckOutput {
    /// Output title.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Markdown summary.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// Markdown details.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

/// Forge check details. `pipeline` remains an open envelope for non-GitHub adapters.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckDetails {
    /// Check-run identifier.
    pub check_run_id: u64,
    /// Workflow-run identifier.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workflow_run_id: Option<u64>,
    /// Check name.
    pub name: String,
    /// Forge state.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// Forge conclusion.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conclusion: Option<String>,
    /// Browser URL.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Additional details URL.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details_url: Option<String>,
    /// Check output.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<CheckOutput>,
    /// Check annotations.
    pub annotations: Vec<CheckAnnotation>,
    /// Failed jobs in the workflow.
    pub failed_jobs: Vec<CheckFailedJob>,
    /// Whether annotations, jobs, or logs were truncated.
    pub truncated: bool,
    /// Structured pipeline for pipeline-oriented forges.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pipeline: Option<Value>,
}

/// Check-details response.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CheckDetailsResult {
    /// Echoed checkout directory.
    pub cwd: String,
    /// Whether the read completed.
    pub success: bool,
    /// Details on success.
    pub details: Option<CheckDetails>,
    /// Inline checkout-shaped error.
    pub error: Option<crate::protocol::checkout::CheckoutError>,
}

#[cfg(test)]
mod tests;
