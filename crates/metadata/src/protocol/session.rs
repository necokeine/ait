//! Connection event subscriptions and client presence; unrelated to domain Session history.

use serde::{Deserialize, Serialize};

/// Connection request methods owned by metadata.
pub const CAPABILITIES: &[&str] = &["session.events.set_subscription.request"];

/// Event streams with installed producers in the independent server.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum SessionEventKind {
    /// A committed Project descriptor changed or was removed.
    #[serde(rename = "project.update")]
    ProjectUpdate,
    /// A configured Workspace script changed state.
    #[serde(rename = "script_status_update")]
    ScriptStatus,
    /// Workspace setup started, advanced, or finished.
    #[serde(rename = "workspace_setup_progress")]
    WorkspaceSetupProgress,
    /// An operational activity or failure was reported to the client.
    #[serde(rename = "activity_log")]
    ActivityLog,
    /// Provider discovery cache changed after an explicit refresh.
    #[serde(rename = "providers_snapshot_update")]
    ProvidersSnapshot,
    /// Committed Agent completion or failure attention.
    #[serde(rename = "agent_attention_required")]
    AgentAttention,
    /// Native approval or structured input requested by an Agent.
    #[serde(rename = "agent_permission_request")]
    AgentPermissionRequest,
    /// Native approval answered or withdrawn.
    #[serde(rename = "agent_permission_resolved")]
    AgentPermissionResolved,
    /// A native child Agent descriptor changed.
    #[serde(rename = "agent.provider_subagents.update")]
    ProviderSubagents,
    /// Terminal hook completion or input attention.
    #[serde(rename = "terminal_attention_required")]
    TerminalAttention,
    /// Observed checkout state changed after a background remote refresh.
    #[serde(rename = "checkout_status_update")]
    CheckoutStatus,
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
            Self::ProjectUpdate => "project.update",
            Self::ScriptStatus => "script_status_update",
            Self::WorkspaceSetupProgress => "workspace_setup_progress",
            Self::ActivityLog => "activity_log",
            Self::ProvidersSnapshot => "providers_snapshot_update",
            Self::AgentAttention => "agent_attention_required",
            Self::AgentPermissionRequest => "agent_permission_request",
            Self::AgentPermissionResolved => "agent_permission_resolved",
            Self::ProviderSubagents => "agent.provider_subagents.update",
            Self::TerminalAttention => "terminal_attention_required",
            Self::CheckoutStatus => "checkout.status.update",
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
