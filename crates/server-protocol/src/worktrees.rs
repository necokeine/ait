//! Paseo worktree request and response payloads exposed under canonical method names.

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

use crate::workspace::WorkspaceDescriptorPayload;

/// Canonical worktree methods implemented by the independent server.
pub const CAPABILITIES: &[&str] = &[
    "workspace.worktree.list.request",
    "workspace.worktree.create.request",
    "workspace.worktree.archive.request",
];

/// List server-managed worktrees for one repository.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorktreeListRequest {
    /// Any directory inside the repository.
    #[serde(default)]
    pub cwd: Option<String>,
    /// Compatibility spelling that takes precedence over `cwd`.
    #[serde(default)]
    pub repo_root: Option<String>,
}

/// One server-managed linked checkout.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorktreeListEntry {
    /// Absolute linked checkout root.
    pub worktree_path: String,
    /// Filesystem creation time, or the Unix epoch when it cannot be read.
    pub created_at: String,
    /// Checked-out local branch; detached worktrees serialize null.
    pub branch_name: Option<String>,
    /// Checked-out commit object name.
    pub head: Option<String>,
}

/// Error shape used by Paseo checkout and worktree RPCs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CheckoutError {
    /// Stable Paseo error category.
    pub code: CheckoutErrorCode,
    /// Safe diagnostic message.
    pub message: String,
}

/// Paseo checkout error categories.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum CheckoutErrorCode {
    /// The selected path does not belong to a Git repository.
    NotGitRepo,
    /// The target is outside the server-owned worktree root.
    NotAllowed,
    /// Git reported a merge conflict.
    MergeConflict,
    /// Another Git, filesystem, or registry error occurred.
    Unknown,
}

/// Worktree list outcome.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WorktreeListResult {
    /// Managed worktrees, in Git's worktree-list order.
    pub worktrees: Vec<WorktreeListEntry>,
    /// Inline Paseo checkout error.
    pub error: Option<CheckoutError>,
}

/// Archive granularity.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorktreeArchiveScope {
    /// Archive one workspace record and delete the worktree after its last active reference.
    #[default]
    Workspace,
    /// Archive every active workspace in the worktree and then delete it.
    Worktree,
}

/// Archive a workspace record or a complete server-owned worktree.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorktreeArchiveRequest {
    /// Exact worktree or descendant workspace directory.
    #[serde(default)]
    pub worktree_path: Option<String>,
    /// Main repository root used with `branchName`.
    #[serde(default)]
    pub repo_root: Option<String>,
    /// Branch used to find a managed worktree.
    #[serde(default)]
    pub branch_name: Option<String>,
    /// Exact workspace record to archive.
    #[serde(default)]
    pub workspace_id: Option<String>,
    /// Archive granularity; omission retains Paseo's workspace default.
    #[serde(default)]
    pub scope: WorktreeArchiveScope,
    /// Legacy compatibility field. Removal is derived from scope and active references.
    #[serde(default)]
    pub delete_worktree_from_disk: bool,
}

/// Worktree archive outcome.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorktreeArchiveResult {
    /// Whether the requested archive operation completed.
    pub success: bool,
    /// Agent identities archived by Paseo; empty until the independent Agent runtime is ported.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub removed_agents: Option<Vec<String>>,
    /// Inline Paseo checkout error.
    pub error: Option<CheckoutError>,
}

/// Worktree creation action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WorktreeCreateAction {
    /// Create a new branch from a base ref.
    BranchOff,
    /// Check out an existing branch.
    Checkout,
}

/// A forge change request selected as a checkout source.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangeRequestCheckoutSource {
    /// Discriminator retained from Paseo's schema.
    pub kind: ChangeRequestCheckoutKind,
    /// Optional forge identifier.
    #[serde(default)]
    pub forge: Option<String>,
    /// Positive change-request number.
    #[serde(deserialize_with = "positive_u64")]
    pub number: u64,
    /// Optional forge project path.
    #[serde(default)]
    pub project_path: Option<String>,
}

/// Checkout-source discriminator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeRequestCheckoutKind {
    /// A pull or merge request.
    ChangeRequest,
}

/// Prompt context used by the first Agent after worktree creation.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct FirstAgentContext {
    /// Optional initial prompt.
    #[serde(default)]
    pub prompt: Option<String>,
    /// Paseo-normalized attachments.
    #[serde(default, deserialize_with = "attachments")]
    pub attachments: Vec<Value>,
}

/// Create and register one server-owned linked worktree.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorktreeCreateRequest {
    /// Source checkout directory, possibly below its Git root.
    pub cwd: String,
    /// Optional active project record to own the workspace.
    #[serde(default)]
    pub project_id: Option<String>,
    /// Optional user-facing worktree directory seed.
    #[serde(default)]
    pub worktree_slug: Option<String>,
    /// Legacy prompt text used when `firstAgentContext` is absent.
    #[serde(default)]
    pub name_context: Option<String>,
    /// Legacy attachments used when `firstAgentContext` is absent.
    #[serde(default, deserialize_with = "optional_attachments")]
    pub attachments: Option<Vec<Value>>,
    /// Current first-Agent prompt context.
    #[serde(default)]
    pub first_agent_context: Option<FirstAgentContext>,
    /// Base ref for branch-off, or target branch for checkout.
    #[serde(default)]
    pub ref_name: Option<String>,
    /// Creation action; omission defaults to branch-off.
    #[serde(default)]
    pub action: Option<WorktreeCreateAction>,
    /// Forge-neutral change-request checkout source.
    #[serde(default)]
    pub checkout_source: Option<ChangeRequestCheckoutSource>,
    /// Legacy GitHub pull-request number.
    #[serde(default, deserialize_with = "optional_positive_u64")]
    pub github_pr_number: Option<u64>,
}

impl WorktreeCreateRequest {
    /// Resolve current and legacy first-Agent context fields as Paseo does.
    #[must_use]
    pub fn normalized_first_agent_context(&self) -> Option<FirstAgentContext> {
        if let Some(context) = &self.first_agent_context {
            return Some(context.clone());
        }
        if self.attachments.is_some() || self.name_context.is_some() {
            return Some(FirstAgentContext {
                prompt: self.name_context.clone(),
                attachments: self.attachments.clone().unwrap_or_default(),
            });
        }
        None
    }
}

/// Worktree creation outcome.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorktreeCreateResult {
    /// Newly registered workspace descriptor.
    pub workspace: Option<WorkspaceDescriptorPayload>,
    /// Inline error text.
    pub error: Option<String>,
    /// Stable worktree-specific error code.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    /// Setup is asynchronous in Paseo; no terminal is created by this slice.
    pub setup_terminal_id: Option<String>,
    /// Reason automation was intentionally skipped.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub setup_skipped_reason: Option<String>,
}

fn positive_u64<'de, D: Deserializer<'de>>(deserializer: D) -> Result<u64, D::Error> {
    let number = u64::deserialize(deserializer)?;
    if number == 0 {
        return Err(serde::de::Error::custom("expected a positive integer"));
    }
    Ok(number)
}

fn optional_positive_u64<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<u64>, D::Error> {
    Option::<u64>::deserialize(deserializer)?.map_or(Ok(None), |number| {
        if number == 0 {
            Err(serde::de::Error::custom("expected a positive integer"))
        } else {
            Ok(Some(number))
        }
    })
}

fn attachments<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<Value>, D::Error> {
    let value = Option::<Value>::deserialize(deserializer)?;
    Ok(normalize_attachments(value))
}

fn optional_attachments<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Vec<Value>>, D::Error> {
    let value = Option::<Value>::deserialize(deserializer)?;
    Ok(Some(normalize_attachments(value)))
}

fn normalize_attachments(value: Option<Value>) -> Vec<Value> {
    value
        .and_then(|value| value.as_array().cloned())
        .unwrap_or_default()
        .into_iter()
        .filter(valid_attachment)
        .collect()
}

fn valid_attachment(value: &Value) -> bool {
    let Some(object) = value.as_object() else {
        return false;
    };
    let Some(kind) = object.get("type").and_then(Value::as_str) else {
        return false;
    };
    match kind {
        "forge_change_request" => {
            literal(object, "mimeType", "application/paseo-forge-change-request")
                && positive(object, "number")
                && strings(object, &["title", "url"])
        }
        "forge_issue" => {
            literal(object, "mimeType", "application/paseo-forge-issue")
                && positive(object, "number")
                && strings(object, &["title", "url"])
        }
        "github_pr" => {
            literal(object, "mimeType", "application/github-pr")
                && positive(object, "number")
                && strings(object, &["title", "url"])
        }
        "github_issue" => {
            literal(object, "mimeType", "application/github-issue")
                && positive(object, "number")
                && strings(object, &["title", "url"])
        }
        "text" => literal(object, "mimeType", "text/plain") && strings(object, &["text"]),
        "review" => {
            literal(object, "mimeType", "application/paseo-review")
                && strings(object, &["cwd", "mode"])
                && matches!(
                    object.get("mode").and_then(Value::as_str),
                    Some("uncommitted" | "base")
                )
                && object.get("comments").is_some_and(Value::is_array)
        }
        "uploaded_file" => {
            strings(object, &["id", "fileName", "mimeType", "path"])
                && object.get("size").and_then(Value::as_u64).is_some()
        }
        _ => false,
    }
}

fn literal(object: &serde_json::Map<String, Value>, key: &str, expected: &str) -> bool {
    object.get(key).and_then(Value::as_str) == Some(expected)
}

fn strings(object: &serde_json::Map<String, Value>, keys: &[&str]) -> bool {
    keys.iter()
        .all(|key| object.get(*key).is_some_and(Value::is_string))
}

fn positive(object: &serde_json::Map<String, Value>, key: &str) -> bool {
    object
        .get(key)
        .and_then(Value::as_u64)
        .is_some_and(|value| value > 0)
}

#[cfg(test)]
mod tests;
