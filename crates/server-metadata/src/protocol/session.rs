//! Connection event subscriptions and client presence; unrelated to domain Session history.

use serde::{Deserialize, Serialize};

/// Connection request methods owned by metadata.
pub const CAPABILITIES: &[&str] = &["session.events.set_subscription.request"];

/// Event streams with installed producers in the independent server.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum SessionEventKind {
    /// Provider discovery cache changed after an explicit refresh.
    #[serde(rename = "providers_snapshot_update")]
    ProvidersSnapshot,
    /// Committed Agent completion or failure attention.
    #[serde(rename = "agent_attention_required")]
    AgentAttention,
    /// Server lifecycle and public metadata changed.
    #[serde(rename = "status.server_info")]
    ServerInfo,
    /// Mutable daemon configuration was saved or reloaded.
    #[serde(rename = "status.daemon_config_changed")]
    DaemonConfig,
}

impl SessionEventKind {
    /// Return the canonical outbound method for this event category.
    #[must_use]
    pub const fn method(self) -> &'static str {
        match self {
            Self::ProvidersSnapshot => "providers_snapshot_update",
            Self::AgentAttention => "agent_attention_required",
            Self::ServerInfo => "status.server_info",
            Self::DaemonConfig => "status.daemon_config_changed",
        }
    }
}

/// A new independently releasable subscription; no directory or history bootstrap is implied.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EventsRequest {
    /// Event categories. Unsupported producer categories are explicitly rejected.
    pub events: Vec<String>,
    /// Whether this subscription may request an in-app notification.
    #[serde(default)]
    pub notifications: bool,
}

/// Source device declared by a connection heartbeat.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DeviceType {
    /// Browser or desktop web client.
    Web,
    /// Mobile client.
    Mobile,
}

/// Paseo-compatible activity heartbeat. Receipt has no acknowledgement and does not renew a Run.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Heartbeat {
    /// Client device type.
    pub device_type: DeviceType,
    /// Currently focused Agent, or null.
    pub focused_agent_id: Option<String>,
    /// Currently focused terminal, or null for older clients.
    pub focused_terminal_id: Option<String>,
    /// RFC3339 timestamp of the client's latest activity.
    pub last_activity_at: String,
    /// Whether the app is visible.
    pub app_visible: bool,
    /// Optional RFC3339 visibility-change timestamp, validated for protocol compatibility.
    pub app_visibility_changed_at: Option<String>,
}
