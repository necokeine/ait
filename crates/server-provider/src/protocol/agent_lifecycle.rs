//! Canonical WebSocket payloads for Paseo Agent runtime directory and metadata lifecycle.

use std::collections::BTreeMap;

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use server_metadata::protocol::workspace::ProjectPlacementPayload;

/// Agent runtime methods backed by the durable runtime registry.
pub const CAPABILITIES: &[&str] = &[
    "agent.list.request",
    "agent.history.get.request",
    "agent.get.request",
    "agent.update.request",
    "agent.archive.request",
    "agent.delete.request",
    "agent.detach.request",
    "agent.attention.clear.request",
    "agent.items.close.request",
];

/// Paseo Agent lifecycle status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentStatus {
    /// Provider construction has started.
    Initializing,
    /// No provider turn is active.
    Idle,
    /// A provider turn is active.
    Running,
    /// The latest provider operation failed.
    Error,
    /// Only a persisted snapshot remains.
    Closed,
}

/// Why a client should surface an Agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentAttentionReason {
    /// A turn finished.
    Finished,
    /// A turn failed.
    Error,
    /// A provider permission is pending.
    Permission,
}

/// Provider capability flags attached to one Agent snapshot.
///
/// Paseo deliberately permits provider-specific boolean keys in addition to its common flags.
pub type AgentCapabilityFlags = BTreeMap<String, bool>;

/// Provider resume handle exposed only when its provider is installed.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentPersistenceHandle {
    /// Provider identifier.
    pub provider: String,
    /// Provider session identifier.
    pub session_id: String,
    /// Optional provider-native handle.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub native_handle: Option<Value>,
    /// Provider-owned metadata.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<BTreeMap<String, Value>>,
}

/// Last runtime facts reported by a provider.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentRuntimeInfo {
    /// Provider identifier.
    pub provider: String,
    /// Current provider session.
    pub session_id: Option<String>,
    /// Effective provider model.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Effective thinking option.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking_option_id: Option<String>,
    /// Effective provider mode.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode_id: Option<String>,
    /// Provider-owned runtime values.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extra: Option<BTreeMap<String, Value>>,
}

/// Paseo-compatible projection of a durable Agent snapshot.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentSnapshotPayload {
    /// Stable Agent identity.
    pub id: String,
    /// Provider identifier.
    pub provider: String,
    /// Session working directory.
    pub cwd: String,
    /// Owning workspace.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
    /// Configured model.
    pub model: Option<String>,
    /// Provider feature descriptors.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub features: Vec<Value>,
    /// Configured thinking option.
    pub thinking_option_id: Option<String>,
    /// Effective thinking option.
    pub effective_thinking_option_id: Option<String>,
    /// Creation timestamp.
    pub created_at: String,
    /// Latest update timestamp.
    pub updated_at: String,
    /// Latest user-message timestamp.
    pub last_user_message_at: Option<String>,
    /// Durable lifecycle state.
    pub status: AgentStatus,
    /// Active turn; persisted records have no active turn.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_turn: Option<Value>,
    /// Provider capability projection.
    pub capabilities: AgentCapabilityFlags,
    /// Current provider mode.
    pub current_mode_id: Option<String>,
    /// Available provider modes.
    pub available_modes: Vec<Value>,
    /// Pending provider permissions.
    pub pending_permissions: Vec<Value>,
    /// Durable provider identity, if its provider is available.
    pub persistence: Option<AgentPersistenceHandle>,
    /// Last provider runtime facts.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runtime_info: Option<AgentRuntimeInfo>,
    /// Last provider error.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    /// User-visible title.
    pub title: Option<String>,
    /// String labels.
    pub labels: BTreeMap<String, String>,
    /// Whether the Agent asks for attention.
    pub requires_attention: bool,
    /// Attention category.
    pub attention_reason: Option<AgentAttentionReason>,
    /// Attention timestamp.
    pub attention_timestamp: Option<String>,
    /// Soft-delete timestamp.
    pub archived_at: Option<String>,
    /// Whether the referenced provider is unavailable.
    pub provider_unavailable: bool,
}

/// Agent directory filters.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentDirectoryFilter {
    /// Require exact key/value label matches.
    #[serde(default)]
    pub labels: Option<BTreeMap<String, String>>,
    /// Restrict placement to project keys.
    #[serde(default)]
    pub project_keys: Option<Vec<String>>,
    /// Restrict lifecycle states.
    #[serde(default)]
    pub statuses: Option<Vec<AgentStatus>>,
    /// Include soft-deleted Agents.
    #[serde(default)]
    pub include_archived: Option<bool>,
    /// Restrict attention state.
    #[serde(default)]
    pub requires_attention: Option<bool>,
    /// Restrict configured thinking option; explicit null means provider default.
    #[serde(default, deserialize_with = "present")]
    pub thinking_option_id: Option<Option<String>>,
}

/// Sortable Agent directory fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentSortKey {
    /// Attention and lifecycle priority.
    StatusPriority,
    /// Creation timestamp.
    CreatedAt,
    /// Update timestamp.
    UpdatedAt,
    /// Case-insensitive title.
    Title,
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

/// One Agent directory sort term.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub struct AgentSort {
    /// Sort field.
    pub key: AgentSortKey,
    /// Sort direction.
    pub direction: SortDirection,
}

/// Cursor page request.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct AgentPageRequest {
    /// Page size, from one through 200.
    pub limit: usize,
    /// Opaque continuation cursor.
    #[serde(default)]
    pub cursor: Option<String>,
}

/// Active directory read request.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentListRequest {
    /// Paseo accepts only the literal `active`.
    #[serde(default)]
    pub scope: Option<String>,
    /// Directory filters.
    #[serde(default)]
    pub filter: Option<AgentDirectoryFilter>,
    /// Ordered sort terms.
    #[serde(default)]
    pub sort: Option<Vec<AgentSort>>,
    /// Cursor page.
    #[serde(default)]
    pub page: Option<AgentPageRequest>,
    /// Subscription request reserved for the later event phase.
    #[serde(default)]
    pub subscribe: Option<Value>,
    /// Sequenced synchronization request reserved for the later event phase.
    #[serde(default)]
    pub sync: Option<Value>,
}

/// Historical Agent directory request.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentHistoryRequest {
    /// Directory filters; archived records are included by default.
    #[serde(default)]
    pub filter: Option<AgentDirectoryFilter>,
    /// Case-insensitive query over title and placement names.
    #[serde(default)]
    pub search: Option<String>,
    /// Ordered sort terms.
    #[serde(default)]
    pub sort: Option<Vec<AgentSort>>,
    /// Cursor page.
    #[serde(default)]
    pub page: Option<AgentPageRequest>,
}

/// One directory row.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentDirectoryEntry {
    /// Agent runtime snapshot.
    pub agent: AgentSnapshotPayload,
    /// Project/workspace placement.
    pub project: ProjectPlacementPayload,
}

/// Directory page metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentPageInfo {
    /// Next page cursor.
    pub next_cursor: Option<String>,
    /// Cursor used to read this page.
    pub prev_cursor: Option<String>,
    /// Whether another page exists.
    pub has_more: bool,
}

/// Agent directory or history result.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentDirectoryResult {
    /// Matching rows.
    pub entries: Vec<AgentDirectoryEntry>,
    /// Page metadata.
    pub page_info: AgentPageInfo,
}

/// Resolve one Agent by full ID, unique prefix, or exact title.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentGetRequest {
    /// Agent identifier accepted by Paseo resolution rules.
    pub agent_id: String,
}

/// One Agent lookup result.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AgentGetResult {
    /// Agent snapshot, or null on lookup failure.
    pub agent: Option<AgentSnapshotPayload>,
    /// Placement, when the Agent belongs to a known workspace.
    pub project: Option<ProjectPlacementPayload>,
    /// Safe lookup error.
    pub error: Option<String>,
}

/// Update Agent metadata.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentUpdateRequest {
    /// Full Agent identity.
    pub agent_id: String,
    /// Nonempty title after trimming.
    #[serde(default)]
    pub name: Option<String>,
    /// Replacement labels when nonempty.
    #[serde(default)]
    pub labels: Option<BTreeMap<String, String>>,
}

/// Common metadata action result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentActionResult {
    /// Full Agent identity.
    pub agent_id: String,
    /// Whether the operation was accepted.
    pub accepted: bool,
    /// Safe business error.
    pub error: Option<String>,
}

/// Request targeting one full Agent identity.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentIdRequest {
    /// Full Agent identity.
    pub agent_id: String,
}

/// Archive result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentArchiveResult {
    /// Full Agent identity.
    pub agent_id: String,
    /// Archive timestamp.
    pub archived_at: String,
}

/// Permanent deletion result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentDeleteResult {
    /// Full Agent identity.
    pub agent_id: String,
}

/// One or several Agent identities accepted by clear-attention.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(untagged)]
pub enum AgentIdSelection {
    /// One Agent identity.
    One(String),
    /// Several Agent identities.
    Many(Vec<String>),
}

/// Clear attention request.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentAttentionClearRequest {
    /// One Agent identity or an array of identities.
    pub agent_id: AgentIdSelection,
}

/// Clear attention result.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentAttentionClearResult {
    /// Original selection.
    pub agent_id: AgentIdSelection,
    /// Updated snapshots.
    pub agents: Vec<AgentSnapshotPayload>,
}

/// Batch close request.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentItemsCloseRequest {
    /// Agents to archive independently.
    #[serde(default)]
    pub agent_ids: Vec<String>,
    /// Terminal identities reserved for the terminal phase.
    #[serde(default)]
    pub terminal_ids: Vec<String>,
}

/// One successful Agent archive in a batch close.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClosedAgentResult {
    /// Agent identity.
    pub agent_id: String,
    /// Archive timestamp.
    pub archived_at: String,
}

/// Batch close result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AgentItemsCloseResult {
    /// Successfully archived Agents; failures are omitted as in Paseo.
    pub agents: Vec<ClosedAgentResult>,
    /// Terminal results. This phase accepts only an empty terminal request.
    pub terminals: Vec<Value>,
}

fn present<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    T::deserialize(deserializer).map(Some)
}

#[cfg(test)]
mod tests;
