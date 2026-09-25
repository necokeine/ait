//! Durable creation receipts and their connection-owned observers.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Creation observation method owned by metadata.
pub const CAPABILITIES: &[&str] = &["creation.subscribe.request"];

/// Resource whose creation is observed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// Workspace provisioning.
    Workspace,
    /// Native Agent provisioning.
    Agent,
}

impl Kind {
    /// Return the wire name used by receipts and update events.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Workspace => "workspace",
            Self::Agent => "agent",
        }
    }
}

/// Observe one idempotency key, optionally without retaining a subscription.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SubscribeRequest {
    /// Resource kind.
    pub kind: Kind,
    /// Explicit durable key supplied to creation.
    pub idempotency_key: String,
    /// Defaults to true.
    pub subscribe: Option<bool>,
}

/// Last committed progress of one creation intent.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    /// Resource kind.
    pub kind: Kind,
    /// Original durable key.
    pub idempotency_key: String,
    /// Monotonically increasing committed revision.
    pub revision: u64,
    /// Accepted, resource-ready, completed or failed.
    pub phase: String,
    /// Reserved or completed Workspace ID.
    pub workspace_id: Option<String>,
    /// Reserved or completed Agent ID.
    pub agent_id: Option<String>,
    /// Safe terminal error, never raw native diagnostics.
    pub error: Option<String>,
    /// Whether an interrupted attempt may have created native resources.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub outcome_unknown: bool,
    /// Completed Workspace projection.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace: Option<Value>,
    /// Completed Agent projection.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent: Option<Value>,
}
