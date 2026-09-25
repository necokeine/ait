//! Paseo registry records, translated from workspace-registry.ts at 2c8e8a8.
//! These identities and timestamps intentionally remain strings, including legacy IDs.
//! Rust translation and modifications: see third-party/paseo/NOTICE and LICENSE.

use serde::{Deserialize, Deserializer, Serialize};

/// The two persisted project kinds; the wire protocol separately accepts `directory`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PersistedProjectKind {
    /// A Git-backed project.
    Git,
    /// A directory without Git metadata.
    NonGit,
}

/// A workspace's backing directory classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PersistedWorkspaceKind {
    /// An ordinary repository checkout.
    LocalCheckout,
    /// A linked worktree, whether managed or external.
    Worktree,
    /// A non-Git directory.
    Directory,
}

/// Persisted project identity and user metadata, independent of execution history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PersistedProjectRecord {
    /// Opaque identity; readers also accept historical non-prefixed IDs.
    pub project_id: String,
    /// Registered root spelling; registry comparison does not resolve symlinks.
    pub root_path: String,
    /// Git or non-Git classification.
    pub kind: PersistedProjectKind,
    /// Derived display name, preserved during root reconciliation.
    pub display_name: String,
    /// Optional project grouping key; missing input normalizes to null.
    #[serde(default)]
    pub project_key: Option<String>,
    /// User name override; an empty override still takes precedence.
    #[serde(default)]
    pub custom_name: Option<String>,
    /// Stored custom icon identity.
    #[serde(default)]
    pub custom_icon_revision: Option<String>,
    /// Original creation timestamp string, without schema-level date validation.
    pub created_at: String,
    /// Last update timestamp string.
    pub updated_at: String,
    /// Required nullable archive timestamp.
    #[serde(deserialize_with = "required_nullable")]
    pub archived_at: Option<String>,
}

impl PersistedProjectRecord {
    /// Resolve the user override, falling back to the derived name only for null.
    #[must_use]
    pub fn display_name(&self) -> &str {
        self.custom_name.as_deref().unwrap_or(&self.display_name)
    }
}

/// Provenance that requires an explicit automation trust decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum UntrustedWorkspaceSource {
    /// A checkout obtained from a change request.
    ChangeRequest {
        /// Forge identifier.
        forge: String,
        /// Positive JavaScript-safe integer.
        #[serde(deserialize_with = "positive_integer")]
        number: u64,
        /// Repository supplying the head branch.
        #[serde(rename = "headRepository")]
        head_repository: String,
    },
}

/// Persisted workspace identity and placement; several workspaces can share a project.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PersistedWorkspaceRecord {
    /// Opaque workspace identity, distinct from the project identity.
    pub workspace_id: String,
    /// Owning project identity.
    pub project_id: String,
    /// Selected working directory, possibly below the worktree root.
    pub cwd: String,
    /// Backing placement classification.
    pub kind: PersistedWorkspaceKind,
    /// Derived durable name, independent of later branch observations.
    pub display_name: String,
    /// Explicit workspace title override.
    #[serde(default)]
    pub title: Option<String>,
    /// Git branch identity, independent of the title.
    #[serde(default)]
    pub branch: Option<String>,
    /// Exact backing checkout/worktree root, retained after deletion.
    #[serde(default)]
    pub worktree_root: Option<String>,
    /// Creation-time comparison base, retained on reconciliation and restore.
    #[serde(default)]
    pub base_branch: Option<String>,
    /// Original Paseo field spelling is retained for serialized compatibility.
    #[serde(default)]
    pub is_paseo_owned_worktree: bool,
    /// Main checkout root for a linked worktree.
    #[serde(default)]
    pub main_repo_root: Option<String>,
    /// Original creation timestamp string.
    pub created_at: String,
    /// Last update timestamp string.
    pub updated_at: String,
    /// Required nullable archive timestamp.
    #[serde(deserialize_with = "required_nullable")]
    pub archived_at: Option<String>,
    /// Change request whose automatic archive was consumed.
    #[serde(default)]
    pub auto_archived_change_request_url: Option<String>,
    /// Pin timestamp, missing input normalizes to null.
    #[serde(default)]
    pub pinned_at: Option<String>,
    /// Omitted and empty labels remain distinguishable; null is invalid.
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub labels: Option<Vec<String>>,
    /// Omitted and present provenance remain distinguishable; null is invalid.
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub untrusted_source: Option<UntrustedWorkspaceSource>,
}

impl PersistedWorkspaceRecord {
    /// Resolve the user title without treating an empty title as absent.
    #[must_use]
    pub fn display_name(&self) -> &str {
        self.title.as_deref().unwrap_or(&self.display_name)
    }
}

fn required_nullable<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::deserialize(deserializer)
}

fn present<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    T::deserialize(deserializer).map(Some)
}

fn positive_integer<'de, D: Deserializer<'de>>(deserializer: D) -> Result<u64, D::Error> {
    let value = f64::deserialize(deserializer)?;
    if value.is_finite() && (1.0..=9_007_199_254_740_991.0).contains(&value) && value.fract() == 0.0
    {
        format!("{value:.0}")
            .parse()
            .map_err(serde::de::Error::custom)
    } else {
        Err(serde::de::Error::custom("expected a positive safe integer"))
    }
}

#[cfg(test)]
mod tests;
