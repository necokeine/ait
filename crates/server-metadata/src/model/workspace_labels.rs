//! Durable workspace label values shared by the application and storage ports.

use serde::{Deserialize, Serialize};

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

/// One host-wide label definition. Workspaces persist the display name as their assignment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceLabelDefinition {
    /// Display name and case-preserving identity.
    pub name: String,
    /// Palette color.
    pub color: WorkspaceLabelColor,
}

/// Normalize label identity exactly like Paseo: collapse whitespace, trim, then compare lowercase.
#[must_use]
pub fn normalize_workspace_label_name(name: &str) -> String {
    name.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Case-insensitive normalized catalog key.
#[must_use]
pub fn workspace_label_key(name: &str) -> String {
    normalize_workspace_label_name(name).to_lowercase()
}

#[cfg(test)]
mod tests;
