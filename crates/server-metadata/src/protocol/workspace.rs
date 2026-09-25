//! Paseo workspace wire schemas at 2c8e8a8, translated to Rust/Serde.
//! Rust translation and modifications: see third-party/paseo/NOTICE and LICENSE.

// Option<Option<T>> preserves absent, explicit null, and a value in the source schemas.
#![allow(clippy::option_option)]

use serde::{Deserialize, Serialize};
use serde_json::Number;

mod checkout;
mod serde_fields;
pub use checkout::ProjectCheckoutLitePayload;
use serde_fields::{optional_positive_integer, present, required_nullable};

/// Paseo `ProjectKind` values; legacy wire variants remain accepted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProjectKind {
    /// Serialized as `git`.
    #[serde(rename = "git")]
    Git,
    /// Serialized as `non_git`.
    #[serde(rename = "non_git")]
    NonGit,
    /// Serialized as `directory`.
    #[serde(rename = "directory")]
    Directory,
}

/// Paseo `WorkspaceKind` values; legacy wire variants remain accepted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WorkspaceKind {
    /// Serialized as `directory`.
    #[serde(rename = "directory")]
    Directory,
    /// Serialized as `local_checkout`.
    #[serde(rename = "local_checkout")]
    LocalCheckout,
    /// Serialized as `checkout`.
    #[serde(rename = "checkout")]
    Checkout,
    /// Serialized as `worktree`.
    #[serde(rename = "worktree")]
    Worktree,
}

/// Paseo `WorkspaceStateBucket` values; legacy wire variants remain accepted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WorkspaceStateBucket {
    /// Serialized as `needs_input`.
    #[serde(rename = "needs_input")]
    NeedsInput,
    /// Serialized as `failed`.
    #[serde(rename = "failed")]
    Failed,
    /// Serialized as `running`.
    #[serde(rename = "running")]
    Running,
    /// Serialized as `attention`.
    #[serde(rename = "attention")]
    Attention,
    /// Serialized as `done`.
    #[serde(rename = "done")]
    Done,
}

/// Paseo `WorkspaceScriptType` values; legacy wire variants remain accepted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WorkspaceScriptType {
    /// Serialized as `script`.
    #[serde(rename = "script")]
    Script,
    /// Serialized as `service`.
    #[serde(rename = "service")]
    Service,
}

/// Paseo `WorkspaceScriptLifecycle` values; legacy wire variants remain accepted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WorkspaceScriptLifecycle {
    /// Serialized as `running`.
    #[serde(rename = "running")]
    Running,
    /// Serialized as `stopped`.
    #[serde(rename = "stopped")]
    Stopped,
}

/// Paseo `WorkspaceScriptHealth` values; legacy wire variants remain accepted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WorkspaceScriptHealth {
    /// Serialized as `healthy`.
    #[serde(rename = "healthy")]
    Healthy,
    /// Serialized as `unhealthy`.
    #[serde(rename = "unhealthy")]
    Unhealthy,
}

/// Paseo `CheckStatus` values; legacy wire variants remain accepted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CheckStatus {
    /// Serialized as `success`.
    #[serde(rename = "success")]
    Success,
    /// Serialized as `failure`.
    #[serde(rename = "failure")]
    Failure,
    /// Serialized as `pending`.
    #[serde(rename = "pending")]
    Pending,
    /// Serialized as `skipped`.
    #[serde(rename = "skipped")]
    Skipped,
    /// Serialized as `cancelled`.
    #[serde(rename = "cancelled")]
    Cancelled,
}

/// Paseo `ChecksStatus` values; legacy wire variants remain accepted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChecksStatus {
    /// Serialized as `none`.
    #[serde(rename = "none")]
    None,
    /// Serialized as `pending`.
    #[serde(rename = "pending")]
    Pending,
    /// Serialized as `success`.
    #[serde(rename = "success")]
    Success,
    /// Serialized as `failure`.
    #[serde(rename = "failure")]
    Failure,
}

/// Paseo `ReviewDecision` values; legacy wire variants remain accepted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReviewDecision {
    /// Serialized as `approved`.
    #[serde(rename = "approved")]
    Approved,
    /// Serialized as `changes_requested`.
    #[serde(rename = "changes_requested")]
    ChangesRequested,
    /// Serialized as `pending`.
    #[serde(rename = "pending")]
    Pending,
}

/// Paseo `Mergeable` values; legacy wire variants remain accepted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Mergeable {
    /// Serialized as `MERGEABLE`.
    #[serde(rename = "MERGEABLE")]
    Mergeable,
    /// Serialized as `CONFLICTING`.
    #[serde(rename = "CONFLICTING")]
    Conflicting,
    /// Serialized as `UNKNOWN`.
    #[serde(rename = "UNKNOWN")]
    Unknown,
}

fn service_type() -> WorkspaceScriptType {
    WorkspaceScriptType::Service
}

/// Paseo project descriptor, including legacy compatibility fields.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceProjectDescriptorPayload {
    /// Paseo `projectId` field; see the pinned source schema.
    pub project_id: String,
    /// Paseo `projectKey` field; see the pinned source schema.
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub project_key: Option<String>,
    /// Paseo `projectDisplayName` field; see the pinned source schema.
    pub project_display_name: String,
    /// Paseo `projectCustomName` field; see the pinned source schema.
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub project_custom_name: Option<Option<String>>,
    /// Paseo `projectCustomIconRevision` field; see the pinned source schema.
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub project_custom_icon_revision: Option<Option<String>>,
    /// Paseo `projectIconRevision` field; see the pinned source schema.
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub project_icon_revision: Option<String>,
    /// Paseo `projectRootPath` field; see the pinned source schema.
    pub project_root_path: String,
    /// Paseo `projectKind` field; see the pinned source schema.
    pub project_kind: ProjectKind,
    /// Paseo `syncSeq` field; see the pinned source schema.
    #[serde(
        default,
        deserialize_with = "optional_positive_integer",
        skip_serializing_if = "Option::is_none"
    )]
    pub sync_seq: Option<u64>,
}

/// Rust equivalent of Paseo `ProjectPlacementPayload`, including its compatibility fields.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectPlacementPayload {
    /// Paseo `projectKey` field; see the pinned source schema.
    pub project_key: String,
    /// Paseo `projectName` field; see the pinned source schema.
    pub project_name: String,
    /// Paseo `workspaceName` field; see the pinned source schema.
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub workspace_name: Option<Option<String>>,
    /// Paseo `checkout` field; see the pinned source schema.
    pub checkout: ProjectCheckoutLitePayload,
}

/// Rust equivalent of Paseo `WorkspaceScriptPayload`, including its compatibility fields.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceScriptPayload {
    /// Paseo `scriptName` field; see the pinned source schema.
    pub script_name: String,
    /// Paseo `type` field; see the pinned source schema.
    #[serde(rename = "type", default = "service_type")]
    pub script_type: WorkspaceScriptType,
    /// Paseo `hostname` field; see the pinned source schema.
    pub hostname: String,
    /// Paseo `port` field; see the pinned source schema.
    #[serde(deserialize_with = "serde_fields::nullable_positive_integer")]
    pub port: Option<u64>,
    /// Paseo `localProxyUrl` field; see the pinned source schema.
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub local_proxy_url: Option<Option<String>>,
    /// Paseo `publicProxyUrl` field; see the pinned source schema.
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub public_proxy_url: Option<Option<String>>,
    /// Paseo `proxyUrl` field; see the pinned source schema.
    #[serde(default)]
    pub proxy_url: Option<String>,
    /// Paseo `lifecycle` field; see the pinned source schema.
    pub lifecycle: WorkspaceScriptLifecycle,
    /// Paseo `health` field; see the pinned source schema.
    #[serde(deserialize_with = "required_nullable")]
    pub health: Option<WorkspaceScriptHealth>,
    /// Paseo `exitCode` field; see the pinned source schema.
    #[serde(default)]
    pub exit_code: Option<Number>,
    /// Paseo `terminalId` field; see the pinned source schema.
    #[serde(default)]
    pub terminal_id: Option<String>,
}

/// Rust equivalent of Paseo `AheadBehind`, including its compatibility fields.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AheadBehind {
    /// Paseo `ahead` field; see the pinned source schema.
    pub ahead: Number,
    /// Paseo `behind` field; see the pinned source schema.
    pub behind: Number,
}

/// Rust equivalent of Paseo `WorkspaceGitRuntimePayload`, including its compatibility fields.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceGitRuntimePayload {
    /// Paseo `currentBranch` field; see the pinned source schema.
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub current_branch: Option<Option<String>>,
    /// Paseo `remoteUrl` field; see the pinned source schema.
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub remote_url: Option<Option<String>>,
    /// Paseo `isPaseoOwnedWorktree` field; see the pinned source schema.
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub is_paseo_owned_worktree: Option<bool>,
    /// Paseo `isDirty` field; see the pinned source schema.
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub is_dirty: Option<Option<bool>>,
    /// Paseo `aheadBehind` field; see the pinned source schema.
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub ahead_behind: Option<Option<AheadBehind>>,
    /// Paseo `aheadOfOrigin` field; see the pinned source schema.
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub ahead_of_origin: Option<Option<Number>>,
    /// Paseo `behindOfOrigin` field; see the pinned source schema.
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub behind_of_origin: Option<Option<Number>>,
}

/// Rust equivalent of Paseo `WorkspaceCheck`, including its compatibility fields.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceCheck {
    /// Paseo `name` field; see the pinned source schema.
    pub name: String,
    /// Paseo `status` field; see the pinned source schema.
    pub status: CheckStatus,
    /// Paseo `url` field; see the pinned source schema.
    #[serde(deserialize_with = "required_nullable")]
    pub url: Option<String>,
    /// Paseo `workflow` field; see the pinned source schema.
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub workflow: Option<String>,
    /// Paseo `duration` field; see the pinned source schema.
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub duration: Option<String>,
    /// Paseo `traits` field; see the pinned source schema.
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub traits: Option<Vec<String>>,
}

/// Rust equivalent of Paseo `WorkspacePullRequest`, including its compatibility fields.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspacePullRequest {
    /// Paseo `number` field; see the pinned source schema.
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub number: Option<Number>,
    /// Paseo `url` field; see the pinned source schema.
    pub url: String,
    /// Paseo `title` field; see the pinned source schema.
    pub title: String,
    /// Paseo `state` field; see the pinned source schema.
    pub state: String,
    /// Paseo `baseRefName` field; see the pinned source schema.
    pub base_ref_name: String,
    /// Paseo `headRefName` field; see the pinned source schema.
    pub head_ref_name: String,
    /// Paseo `isMerged` field; see the pinned source schema.
    pub is_merged: bool,
    /// Paseo `isDraft` field; see the pinned source schema.
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub is_draft: Option<bool>,
    /// Paseo `mergeable` field; see the pinned source schema.
    #[serde(
        default,
        deserialize_with = "serde_fields::mergeable",
        skip_serializing_if = "Option::is_none"
    )]
    pub mergeable: Option<Mergeable>,
    /// Paseo `checks` field; see the pinned source schema.
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub checks: Option<Vec<WorkspaceCheck>>,
    /// Paseo `checksStatus` field; see the pinned source schema.
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub checks_status: Option<ChecksStatus>,
    /// Paseo `reviewDecision` field; see the pinned source schema.
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub review_decision: Option<Option<ReviewDecision>>,
    /// Paseo `repoOwner` field; see the pinned source schema.
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub repo_owner: Option<String>,
    /// Paseo `repoName` field; see the pinned source schema.
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub repo_name: Option<String>,
    /// Paseo `github` field; see the pinned source schema.
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub github: Option<serde_json::Value>,
}

/// Rust equivalent of Paseo `WorkspaceRuntimeError`, including its compatibility fields.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceRuntimeError {
    /// Paseo `message` field; see the pinned source schema.
    pub message: String,
}

/// Rust equivalent of Paseo `WorkspaceGitHubRuntimePayload`, including its compatibility fields.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceGitHubRuntimePayload {
    /// Paseo `featuresEnabled` field; see the pinned source schema.
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub features_enabled: Option<bool>,
    /// Paseo `pullRequest` field; see the pinned source schema.
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub pull_request: Option<Option<WorkspacePullRequest>>,
    /// Paseo `error` field; see the pinned source schema.
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub error: Option<Option<WorkspaceRuntimeError>>,
    /// Paseo `refreshedAt` field; see the pinned source schema.
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub refreshed_at: Option<Option<String>>,
}

/// Rust equivalent of Paseo `DiffStat`, including its compatibility fields.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiffStat {
    /// Paseo `additions` field; see the pinned source schema.
    pub additions: Number,
    /// Paseo `deletions` field; see the pinned source schema.
    pub deletions: Number,
}

/// Rust equivalent of Paseo `WorkspaceDescriptorPayload`, including its compatibility fields.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(from = "WorkspaceDescriptorInput")]
pub struct WorkspaceDescriptorPayload {
    /// Paseo `id` field; see the pinned source schema.
    pub id: String,
    /// Paseo `projectId` field; see the pinned source schema.
    pub project_id: String,
    /// Paseo `projectDisplayName` field; see the pinned source schema.
    pub project_display_name: String,
    /// Paseo `projectCustomName` field; see the pinned source schema.
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub project_custom_name: Option<Option<String>>,
    /// Paseo `projectCustomIconRevision` field; see the pinned source schema.
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub project_custom_icon_revision: Option<Option<String>>,
    /// Paseo `projectRootPath` field; see the pinned source schema.
    pub project_root_path: String,
    /// Paseo `workspaceDirectory` field; see the pinned source schema.
    pub workspace_directory: String,
    /// Paseo `worktreeSlug` field; see the pinned source schema.
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub worktree_slug: Option<String>,
    /// Paseo `projectKind` field; see the pinned source schema.
    pub project_kind: ProjectKind,
    /// Paseo `workspaceKind` field; see the pinned source schema.
    pub workspace_kind: WorkspaceKind,
    /// Paseo `name` field; see the pinned source schema.
    pub name: String,
    /// Paseo `title` field; see the pinned source schema.
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub title: Option<Option<String>>,
    /// Paseo `pinnedAt` field; see the pinned source schema.
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub pinned_at: Option<Option<String>>,
    /// Paseo `labels` field; see the pinned source schema.
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub labels: Option<Vec<String>>,
    /// Paseo `archivingAt` field; see the pinned source schema.
    #[serde(default)]
    pub archiving_at: Option<String>,
    /// Paseo `status` field; see the pinned source schema.
    pub status: WorkspaceStateBucket,
    /// Paseo `statusEnteredAt` field; see the pinned source schema.
    #[serde(default)]
    pub status_entered_at: Option<String>,
    /// Paseo `activityAt` field; see the pinned source schema.
    #[serde(deserialize_with = "required_nullable")]
    pub activity_at: Option<String>,
    /// Paseo `diffStat` field; see the pinned source schema.
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub diff_stat: Option<Option<DiffStat>>,
    /// Paseo `scripts` field; see the pinned source schema.
    #[serde(default)]
    pub scripts: Vec<WorkspaceScriptPayload>,
    /// Paseo `gitRuntime` field; see the pinned source schema.
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub git_runtime: Option<Option<WorkspaceGitRuntimePayload>>,
    /// Paseo `githubRuntime` field; see the pinned source schema.
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub github_runtime: Option<Option<WorkspaceGitHubRuntimePayload>>,
    /// Paseo `forge` field; see the pinned source schema.
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub forge: Option<String>,
    /// Paseo `project` field; see the pinned source schema.
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub project: Option<ProjectPlacementPayload>,
    /// Paseo `syncSeq` field; see the pinned source schema.
    #[serde(
        default,
        deserialize_with = "optional_positive_integer",
        skip_serializing_if = "Option::is_none"
    )]
    pub sync_seq: Option<u64>,
}

// Source shape before workspaceDirectory fallback.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WorkspaceDescriptorInput {
    /// Paseo `id` field; see the pinned source schema.
    pub id: String,
    /// Paseo `projectId` field; see the pinned source schema.
    pub project_id: String,
    /// Paseo `projectDisplayName` field; see the pinned source schema.
    pub project_display_name: String,
    /// Paseo `projectCustomName` field; see the pinned source schema.
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub project_custom_name: Option<Option<String>>,
    /// Paseo `projectCustomIconRevision` field; see the pinned source schema.
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub project_custom_icon_revision: Option<Option<String>>,
    /// Paseo `projectRootPath` field; see the pinned source schema.
    pub project_root_path: String,
    /// Paseo `workspaceDirectory` field; see the pinned source schema.
    #[serde(default, deserialize_with = "present")]
    workspace_directory: Option<String>,
    /// Paseo `worktreeSlug` field; see the pinned source schema.
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub worktree_slug: Option<String>,
    /// Paseo `projectKind` field; see the pinned source schema.
    pub project_kind: ProjectKind,
    /// Paseo `workspaceKind` field; see the pinned source schema.
    pub workspace_kind: WorkspaceKind,
    /// Paseo `name` field; see the pinned source schema.
    pub name: String,
    /// Paseo `title` field; see the pinned source schema.
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub title: Option<Option<String>>,
    /// Paseo `pinnedAt` field; see the pinned source schema.
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub pinned_at: Option<Option<String>>,
    /// Paseo `labels` field; see the pinned source schema.
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub labels: Option<Vec<String>>,
    /// Paseo `archivingAt` field; see the pinned source schema.
    #[serde(default)]
    pub archiving_at: Option<String>,
    /// Paseo `status` field; see the pinned source schema.
    pub status: WorkspaceStateBucket,
    /// Paseo `statusEnteredAt` field; see the pinned source schema.
    #[serde(default)]
    pub status_entered_at: Option<String>,
    /// Paseo `activityAt` field; see the pinned source schema.
    #[serde(deserialize_with = "required_nullable")]
    pub activity_at: Option<String>,
    /// Paseo `diffStat` field; see the pinned source schema.
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub diff_stat: Option<Option<DiffStat>>,
    /// Paseo `scripts` field; see the pinned source schema.
    #[serde(default)]
    pub scripts: Vec<WorkspaceScriptPayload>,
    /// Paseo `gitRuntime` field; see the pinned source schema.
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub git_runtime: Option<Option<WorkspaceGitRuntimePayload>>,
    /// Paseo `githubRuntime` field; see the pinned source schema.
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub github_runtime: Option<Option<WorkspaceGitHubRuntimePayload>>,
    /// Paseo `forge` field; see the pinned source schema.
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub forge: Option<String>,
    /// Paseo `project` field; see the pinned source schema.
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub project: Option<ProjectPlacementPayload>,
    /// Paseo `syncSeq` field; see the pinned source schema.
    #[serde(
        default,
        deserialize_with = "optional_positive_integer",
        skip_serializing_if = "Option::is_none"
    )]
    pub sync_seq: Option<u64>,
}

impl From<WorkspaceDescriptorInput> for WorkspaceDescriptorPayload {
    fn from(input: WorkspaceDescriptorInput) -> Self {
        Self {
            id: input.id,
            project_id: input.project_id,
            project_display_name: input.project_display_name,
            project_custom_name: input.project_custom_name,
            project_custom_icon_revision: input.project_custom_icon_revision,
            workspace_directory: input
                .workspace_directory
                .unwrap_or_else(|| input.project_root_path.clone()),
            worktree_slug: input.worktree_slug,
            project_kind: input.project_kind,
            workspace_kind: input.workspace_kind,
            name: input.name,
            title: input.title,
            pinned_at: input.pinned_at,
            labels: input.labels,
            archiving_at: input.archiving_at,
            status: input.status,
            status_entered_at: input.status_entered_at,
            activity_at: input.activity_at,
            diff_stat: input.diff_stat,
            scripts: input.scripts,
            git_runtime: input.git_runtime,
            github_runtime: input.github_runtime,
            forge: input.forge,
            project: input.project,
            sync_seq: input.sync_seq,
            project_root_path: input.project_root_path,
        }
    }
}

#[cfg(test)]
mod tests;
