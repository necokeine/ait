//! Paseo timeline queries and append-only display entries, separate from domain Messages.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Timeline methods installed with the native execution service.
pub const CAPABILITIES: &[&str] = &[
    "agent.timeline.get.request",
    "agent.timeline.search.request",
    "agent.timeline.list_prompts.request",
    "agent.timeline.append.request",
    "agent.timeline.set_subscription.request",
];

/// Stable position in one timeline generation.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Cursor {
    /// Opaque durable generation identity.
    pub epoch: String,
    /// Sequence position; committed rows start at one.
    pub seq: u64,
}

/// Page selection relative to a cursor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    /// Most recent matching rows.
    Tail,
    /// Rows strictly before the cursor.
    Before,
    /// Rows strictly after the cursor.
    After,
}

/// View requested by the client; identity rows currently serve both projections.
#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Projection {
    /// Display projection without destructive rewriting of stored items.
    #[default]
    Projected,
    /// Individual canonical display items.
    Canonical,
}

/// Bounded timeline page query.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FetchRequest {
    /// Registered Agent identifier.
    pub agent_id: String,
    /// Defaults to after with a cursor, otherwise tail.
    pub direction: Option<Direction>,
    /// Optional exclusive boundary.
    pub cursor: Option<Cursor>,
    /// Zero requests the entire window, subject to transport budgets.
    pub limit: Option<usize>,
    /// Requested view.
    #[serde(default)]
    pub projection: Projection,
    /// Echoed merge hint for a client loading discontiguous windows.
    pub merge_window: Option<bool>,
}

/// Case-insensitive text search over user and assistant display messages.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SearchRequest {
    /// Registered Agent identifier.
    pub agent_id: String,
    /// Nonempty search text.
    pub query: String,
    /// Exclusive sequence boundary, defaulting to zero.
    pub cursor: Option<usize>,
}

/// One immutable provider projection item before sequencing.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeItem {
    /// Provider-native stable identity, scoped to its Agent.
    pub key: String,
    /// Native turn identity, when available.
    pub turn_id: Option<String>,
    /// RFC3339 source timestamp.
    pub timestamp: String,
    /// Paseo timeline item; never a host domain Message.
    pub item: Value,
}

/// Plugin display append request; plugin identity is supplied by connection provenance.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AppendRequest {
    /// Registered Agent identifier.
    pub agent_id: String,
    /// Display extension item, never provider prompt history.
    pub item: PluginItem,
}

/// Immutable plugin extension payload.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PluginItem {
    /// Must be `plugin`.
    pub r#type: String,
    /// Stable plugin-local identity used for idempotent append.
    pub id: String,
    /// Plugin-specific display kind.
    pub kind: String,
    /// Positive schema version.
    pub version: u32,
    /// JSON display data, capped at 64 KiB.
    pub data: Value,
}

/// Agent IDs selected by one independently releasable subscription.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SubscriptionRequest {
    /// At most 32 full IDs or unambiguous identifiers.
    pub agent_ids: Vec<String>,
}
