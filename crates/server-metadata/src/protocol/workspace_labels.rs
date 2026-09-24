//! Canonical workspace label request, response, and live update payloads.

use serde::{Deserialize, Serialize};

/// Production methods implemented by the workspace label service.
pub const CAPABILITIES: &[&str] = &[
    "workspace.label.list.request",
    "workspace.label.assignment.set.request",
    "workspace.label.update.request",
    "workspace.label.delete.inspect.request",
    "workspace.label.delete.request",
];

/// Paseo's fixed workspace label palette.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceLabelColor {
    /// Violet.
    Violet,
    /// Sky blue.
    Sky,
    /// Emerald green.
    Emerald,
    /// Orange.
    Orange,
    /// Pink.
    Pink,
    /// Indigo.
    Indigo,
    /// Teal.
    Teal,
    /// Red.
    Red,
    /// Amber.
    Amber,
    /// Blue.
    Blue,
}

/// One host-wide label definition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceLabelDefinition {
    /// Display name.
    pub name: String,
    /// Palette color.
    pub color: WorkspaceLabelColor,
}

/// Optional list subscription request. The standalone server assigns the returned ID.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceLabelSubscribe {
    /// Legacy requested ID accepted by Paseo's schema; modern delivery may replace it.
    #[serde(default)]
    pub subscription_id: Option<String>,
}

/// Incremental synchronization cursor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceLabelSyncCursor {
    /// Process generation.
    pub generation: String,
    /// Last sequence observed by the client.
    pub after_seq: u64,
}

/// List or subscribe to the host label catalog.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceLabelListRequest {
    /// Subscribe after the coherent initial response.
    #[serde(default)]
    pub subscribe: Option<WorkspaceLabelSubscribe>,
    /// Optional incremental cursor.
    #[serde(default)]
    pub sync: Option<WorkspaceLabelSyncCursor>,
}

/// Set one workspace assignment, creating the definition on first assignment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceLabelAssignmentSetRequest {
    /// Active workspace identity.
    pub workspace_id: String,
    /// Requested definition.
    pub label: WorkspaceLabelDefinition,
    /// Whether the label is assigned.
    pub assigned: bool,
}

/// Edit a definition's name, color, or both in one operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceLabelUpdateRequest {
    /// Existing name, compared case-insensitively after normalization.
    pub name: String,
    /// Replacement display name.
    #[serde(default)]
    pub new_name: Option<String>,
    /// Replacement color.
    #[serde(default)]
    pub color: Option<WorkspaceLabelColor>,
}

/// Delete or inspect a definition by name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceLabelDeleteRequest {
    /// Definition name.
    pub name: String,
}

/// Synchronization response mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceLabelSyncMode {
    /// Complete catalog.
    Snapshot,
    /// Compacted changes after the cursor.
    Changes,
}

/// One removal included in a compacted catch-up response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceLabelRemoval {
    /// Removed display name.
    pub name: String,
    /// Removal sequence.
    pub seq: u64,
}

/// Synchronization metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceLabelSyncMetadata {
    /// Snapshot or changes.
    pub mode: WorkspaceLabelSyncMode,
    /// Current process generation.
    pub generation: String,
    /// Current sequence.
    pub head_seq: u64,
    /// Compacted removals.
    pub removals: Vec<WorkspaceLabelRemoval>,
}

/// Label list response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceLabelListResult {
    /// Server-assigned subscription identity when requested.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subscription_id: Option<String>,
    /// Full snapshot or compacted upserts.
    pub labels: Vec<WorkspaceLabelDefinition>,
    /// Synchronization boundary.
    pub sync: WorkspaceLabelSyncMetadata,
}

/// Assignment response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceLabelAssignmentSetResult {
    /// Authoritative definition.
    pub label: WorkspaceLabelDefinition,
    /// Complete workspace assignment list.
    pub workspace_labels: Vec<String>,
}

/// Definition edit response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceLabelUpdateResult {
    /// Updated definition.
    pub label: WorkspaceLabelDefinition,
    /// Workspaces whose assignment name changed.
    pub affected_workspace_count: usize,
}

/// Delete inspection and deletion response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceLabelAffectedResult {
    /// Active and archived workspaces carrying the name.
    pub affected_workspace_count: usize,
}

/// Live catalog update payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum WorkspaceLabelLiveUpdate {
    /// Definition creation or edit.
    Upsert {
        /// Connection-owned subscription identity.
        subscription_id: String,
        /// Current definition.
        label: WorkspaceLabelDefinition,
        /// Previous name for a rename.
        #[serde(skip_serializing_if = "Option::is_none")]
        previous_name: Option<String>,
        /// Process generation.
        generation: String,
        /// Positive sequence.
        seq: u64,
    },
    /// Definition deletion.
    Remove {
        /// Connection-owned subscription identity.
        subscription_id: String,
        /// Deleted display name.
        name: String,
        /// Process generation.
        generation: String,
        /// Positive sequence.
        seq: u64,
    },
}

#[cfg(test)]
mod tests;
