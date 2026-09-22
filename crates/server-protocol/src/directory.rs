//! Project and workspace directory RPC payloads translated from Paseo messages.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::workspace::{WorkspaceDescriptorPayload, WorkspaceProjectDescriptorPayload};

/// Methods whose first implementation is backed by the file registries.
pub const CAPABILITIES: &[&str] = &[
    "project.add.request",
    "project.create_directory.request",
    "project.list.request",
    "project.rename.request",
    "project.remove.request",
    "workspace.open.request",
    "workspace.create.request",
    "workspace.list.request",
    "workspace.archive.request",
    "workspace.title.set.request",
    "workspace.pin.set.request",
];

/// Register an existing directory as a project without creating a workspace.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ProjectAddRequest {
    /// Existing directory selected by the user.
    pub cwd: String,
}

/// Project registration outcome.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectAddResult {
    /// Registered or refreshed project.
    pub project: Option<WorkspaceProjectDescriptorPayload>,
    /// Safe business error.
    pub error: Option<String>,
    /// Stable business error code.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
}

/// Create one child directory and register it as a project.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectCreateDirectoryRequest {
    /// Existing parent directory.
    pub parent_path: String,
    /// Single child name without path separators.
    pub name: String,
}

/// Atomic directory creation and registration outcome.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectCreateDirectoryResult {
    /// Created path, including registration failures after creation.
    pub directory_path: Option<String>,
    /// Registered project.
    pub project: Option<WorkspaceProjectDescriptorPayload>,
    /// Safe business error.
    pub error: Option<String>,
    /// Open-ended Paseo-compatible error code.
    pub error_code: Option<String>,
}

/// Open an existing directory, reusing or restoring its oldest workspace.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct WorkspaceOpenRequest {
    /// Existing directory selected by the user.
    pub cwd: String,
}

/// Workspace open outcome.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceOpenResult {
    /// Reused, restored, or newly created workspace.
    pub workspace: Option<WorkspaceDescriptorPayload>,
    /// Safe business error.
    pub error: Option<String>,
    /// Stable business error code.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
}

/// Backing source for a new workspace.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WorkspaceCreateSource {
    /// Existing local directory; always creates a fresh workspace record.
    Directory {
        /// Existing local path.
        path: String,
        /// Optional active project to own the new workspace.
        #[serde(default, rename = "projectId")]
        project_id: Option<String>,
    },
    /// New linked worktree workflow. The worktree service implements this source later.
    Worktree {
        /// Preserve the source payload for forward-compatible rejection.
        #[serde(flatten)]
        fields: std::collections::BTreeMap<String, Value>,
    },
}

/// Create a fresh workspace from a directory or worktree source.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceCreateRequest {
    /// Optional caller-reserved Paseo workspace identity.
    #[serde(default, deserialize_with = "optional_workspace_id")]
    pub workspace_id: Option<String>,
    /// Optional initial agent request, coordinated by the agent creation service.
    #[serde(default)]
    pub agent: Option<Value>,
    /// Whether the caller requests creation updates.
    #[serde(default)]
    pub subscribe: Option<bool>,
    /// Optional idempotency key for the complete creation pipeline.
    #[serde(default, deserialize_with = "optional_idempotency_key")]
    pub idempotency_key: Option<String>,
    /// Optional explicit title.
    #[serde(default)]
    pub title: Option<String>,
    /// Optional first-agent prompt context.
    #[serde(default)]
    pub first_agent_context: Option<Value>,
    /// Directory or worktree backing source.
    pub source: WorkspaceCreateSource,
}

/// Fresh workspace creation outcome for the registry-backed directory path.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceCreateResult {
    /// Created workspace, or null on a business rejection.
    pub workspace: Option<WorkspaceDescriptorPayload>,
    /// Setup terminal identity; directory creation does not start setup yet.
    pub setup_terminal_id: Option<String>,
    /// Safe business error.
    pub error: Option<String>,
    /// Stable or forward-compatible business error code.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
}

fn optional_workspace_id<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<String>, D::Error> {
    let value = Option::<String>::deserialize(deserializer)?;
    if value.as_deref().is_some_and(|value| {
        value.len() != 20
            || !value.starts_with("wks_")
            || !value[4..]
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    }) {
        return Err(serde::de::Error::custom("invalid workspace identity"));
    }
    Ok(value)
}

fn optional_idempotency_key<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<String>, D::Error> {
    let value = Option::<String>::deserialize(deserializer)?;
    if value
        .as_deref()
        .is_some_and(|value| value.is_empty() || value.len() > 512)
    {
        return Err(serde::de::Error::custom("invalid idempotency key"));
    }
    Ok(value)
}

/// Project list request; directory synchronization is reserved for its dedicated service.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectListRequest {
    /// Paseo directory synchronization cursor, when requested.
    #[serde(default)]
    pub sync: Option<Value>,
}

/// Active project directory snapshot.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectListResult {
    /// Active projects in registry insertion order.
    pub projects: Vec<WorkspaceProjectDescriptorPayload>,
}

/// Rename or clear a project's user-visible name.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectRenameRequest {
    /// Project identity.
    pub project_id: String,
    /// New name; null and whitespace-only values clear the override.
    pub custom_name: Option<String>,
}

/// Project rename outcome, including expected business rejection details.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectRenameResult {
    /// Project identity.
    pub project_id: String,
    /// Whether the registry was changed.
    pub accepted: bool,
    /// Persisted normalized override.
    pub custom_name: Option<String>,
    /// Safe business error; storage errors use the transport error envelope.
    pub error: Option<String>,
}

/// Remove a project and archive each active child workspace.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectRemoveRequest {
    /// Project identity.
    pub project_id: String,
}

/// Project removal outcome.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectRemoveResult {
    /// Requested project identity.
    pub project_id: String,
    /// Whether the operation completed.
    pub accepted: bool,
    /// Child workspace records archived by the operation.
    pub removed_workspace_ids: Vec<String>,
    /// Safe business error.
    pub error: Option<String>,
}

/// Workspace text filter.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceListFilter {
    /// Case-insensitive text query.
    #[serde(default)]
    pub query: Option<String>,
    /// Restrict results to one project.
    #[serde(default)]
    pub project_id: Option<String>,
    /// Accepted Paseo compatibility field; it does not filter results.
    #[serde(default)]
    pub id_prefix: Option<String>,
}

/// Workspace sort key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceSortKey {
    /// Runtime status ordering.
    StatusPriority,
    /// Latest known activity.
    ActivityAt,
    /// Resolved workspace name.
    Name,
    /// Owning project identity.
    ProjectId,
}

/// Sort direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SortDirection {
    /// Ascending order.
    Asc,
    /// Descending order.
    Desc,
}

/// One ordered workspace sort clause.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceSort {
    /// Field to compare.
    pub key: WorkspaceSortKey,
    /// Comparison direction.
    pub direction: SortDirection,
}

/// Bounded workspace page request.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspacePage {
    /// Maximum returned rows, from 1 through 200.
    pub limit: usize,
    /// Opaque continuation cursor.
    #[serde(default)]
    pub cursor: Option<String>,
}

/// Connection-owned subscription request.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubscriptionRequest {
    /// Existing subscription to replace or resume.
    #[serde(default)]
    pub subscription_id: Option<String>,
}

/// Workspace directory snapshot request.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceListRequest {
    /// Optional text/project filter.
    #[serde(default)]
    pub filter: Option<WorkspaceListFilter>,
    /// Ordered sort clauses.
    #[serde(default)]
    pub sort: Option<Vec<WorkspaceSort>>,
    /// Optional bounded page.
    #[serde(default)]
    pub page: Option<WorkspacePage>,
    /// Optional live-update subscription.
    #[serde(default)]
    pub subscribe: Option<SubscriptionRequest>,
    /// Optional directory synchronization cursor.
    #[serde(default)]
    pub sync: Option<Value>,
}

/// Workspace page metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspacePageInfo {
    /// Cursor for the following page.
    pub next_cursor: Option<String>,
    /// Cursor for the preceding page.
    pub prev_cursor: Option<String>,
    /// Whether another page follows this one.
    pub has_more: bool,
}

/// Workspace directory snapshot.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceListResult {
    /// Active workspace descriptors.
    pub entries: Vec<WorkspaceDescriptorPayload>,
    /// Active projects with no active workspaces.
    pub empty_projects: Vec<WorkspaceProjectDescriptorPayload>,
    /// Page cursors and completion state.
    pub page_info: WorkspacePageInfo,
}

/// Archive one workspace.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceArchiveRequest {
    /// Workspace identity.
    pub workspace_id: String,
}

/// Workspace archive outcome.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceArchiveResult {
    /// Workspace identity.
    pub workspace_id: String,
    /// Archive timestamp, or null on rejection.
    pub archived_at: Option<String>,
    /// Safe business error.
    pub error: Option<String>,
}

/// Set or clear a workspace title.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceTitleSetRequest {
    /// Workspace identity.
    pub workspace_id: String,
    /// New title; null and whitespace-only values clear it.
    pub title: Option<String>,
}

/// Workspace title outcome.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceTitleSetResult {
    /// Workspace identity.
    pub workspace_id: String,
    /// Whether the record was found and changed.
    pub accepted: bool,
    /// Persisted normalized title.
    pub title: Option<String>,
    /// Safe business error.
    pub error: Option<String>,
}

/// Set a workspace's pinned state.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspacePinSetRequest {
    /// Workspace identity.
    pub workspace_id: String,
    /// Whether the workspace should be pinned.
    pub pinned: bool,
}

/// Workspace pin outcome.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspacePinSetResult {
    /// Workspace identity.
    pub workspace_id: String,
    /// Whether the record was found and changed.
    pub accepted: bool,
    /// Pin timestamp, or null when unpinned/rejected.
    pub pinned_at: Option<String>,
    /// Safe business error.
    pub error: Option<String>,
}

#[cfg(test)]
mod tests;
