//! Project icon WebSocket payloads translated from Paseo.

use serde::{Deserialize, Serialize};

/// Canonical project icon methods.
pub const CAPABILITIES: &[&str] = &["project.icon.set.request", "project.icon.get.request"];

/// Client-owned icon source. URL fetching is deliberately absent.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ProjectIconSource {
    /// Remove custom bytes and resume automatic discovery.
    Automatic,
    /// Validate and store client-provided base64 image bytes.
    Upload {
        /// Base64-encoded image bytes.
        data: String,
    },
}

/// Set or clear a custom icon for a registered project.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectIconSetRequest {
    /// Project identity.
    pub project_id: String,
    /// Automatic mode or uploaded bytes.
    pub source: ProjectIconSource,
}

/// Read the effective custom or automatically discovered icon.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectIconGetRequest {
    /// Project identity.
    pub project_id: String,
}

/// Base64 project icon returned to clients.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectIconPayload {
    /// Base64-encoded image bytes.
    pub data: String,
    /// MIME type detected by the server.
    pub mime_type: String,
}

/// Project icon mutation outcome.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectIconSetResult {
    /// Project identity.
    pub project_id: String,
    /// Whether the custom/automatic selection was persisted.
    pub accepted: bool,
    /// Safe business error.
    pub error: Option<String>,
}

/// Effective project icon read outcome.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectIconGetResult {
    /// Project identity.
    pub project_id: String,
    /// Effective icon, or null when automatic discovery found none.
    pub icon: Option<ProjectIconPayload>,
    /// Safe business error.
    pub error: Option<String>,
}

#[cfg(test)]
mod tests;
