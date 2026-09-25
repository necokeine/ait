//! Provider discovery and snapshot requests.

use serde::Deserialize;
use serde_json::Value;

/// Implemented discovery methods; diagnostics, usage and recent sessions remain separate.
pub const CAPABILITIES: &[&str] = &[
    "provider.available.list.request",
    "provider.models.list.request",
    "provider.modes.list.request",
    "provider.features.list.request",
    "provider.snapshot.get.request",
    "provider.snapshot.refresh.request",
];

/// Provider-owned model and mode definitions, with no credentials or private diagnostics.
#[derive(Debug, Clone, Default)]
pub struct Details {
    /// Paseo model definitions fetched from the native provider.
    pub models: Vec<Value>,
    /// Modes actually supported by this server adapter.
    pub modes: Vec<Value>,
    /// Runtime toggles and selects supported by the adapter.
    pub features: Vec<Value>,
}

/// Select one installed Provider in an optional working directory.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ListRequest {
    /// Provider identity.
    pub provider: String,
    /// Existing absolute directory; defaults to the server's working directory.
    pub cwd: Option<String>,
}

/// Fetch cached discovery facts, optionally reusing an unchanged snapshot.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SnapshotRequest {
    /// Scope used by native provider configuration lookup.
    pub cwd: Option<String>,
    /// Previously observed content hash.
    pub if_none_match: Option<String>,
}

/// Refresh all installed Providers, or a selected subset, for one directory.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RefreshRequest {
    /// Scope used by native provider configuration lookup.
    pub cwd: Option<String>,
    /// Omitted means every registered adapter.
    pub providers: Option<Vec<String>>,
}
