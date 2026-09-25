//! Existing native session discovery, import, refresh and context export.

use std::collections::BTreeMap;

use serde::Deserialize;

use super::timeline::Cursor;

/// Methods implemented by the native session worker.
pub const CAPABILITIES: &[&str] = &[
    "provider.sessions.recent.list.request",
    "agent.import.request",
    "agent.refresh.request",
    "agent.fork_context.request",
];

/// Filters for recent sessions which have not been actively imported.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecentRequest {
    /// Optional exact working directory.
    pub cwd: Option<String>,
    /// Registered providers, or all when omitted.
    pub providers: Option<Vec<String>>,
    /// Inclusive RFC3339 activity boundary.
    pub since: Option<String>,
    /// Positive result limit, at most 200; defaults to 20.
    pub limit: Option<usize>,
    /// Case-insensitive substring of title, prompt preview, handle or directory.
    pub query: Option<String>,
}

/// Import an existing provider handle. Legacy field aliases must agree when both are supplied.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ImportRequest {
    /// Provider identity.
    pub provider_id: Option<String>,
    /// Legacy provider identity.
    pub provider: Option<String>,
    /// Native session identity.
    pub provider_handle_id: Option<String>,
    /// Legacy native identity.
    pub session_id: Option<String>,
    /// Existing absolute directory, checked against the native session.
    pub cwd: String,
    /// Explicit matching Workspace; omitted uses metadata's directory opening service.
    pub workspace_id: Option<String>,
    /// User-visible Agent labels.
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
}

/// Select all context, or an inclusive timeline boundary.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ForkRequest {
    /// Full Agent ID, unique prefix or title.
    pub agent_id: String,
    /// Preferred inclusive sequence boundary.
    pub boundary_cursor: Option<Cursor>,
    /// Alternative inclusive assistant-message boundary.
    pub boundary_message_id: Option<String>,
}
