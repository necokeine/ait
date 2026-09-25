//! Read-only native session facts; the provider remains the owner of its history.

use serde::Serialize;
use server_domain::agent_runtime::StoredAgentConfig;

use crate::protocol::timeline::NativeItem;

/// Paseo-compatible discovery descriptor, without local native storage paths or credentials.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionDescriptor {
    /// Registered adapter identity.
    pub provider_id: String,
    /// User-facing adapter label.
    pub provider_label: String,
    /// Native session identity.
    pub provider_handle_id: String,
    /// Native working directory.
    pub cwd: String,
    /// Native title, when present.
    pub title: Option<String>,
    /// First user text, when supplied by native discovery.
    pub first_prompt_preview: Option<String>,
    /// Last user text, when supplied by native discovery.
    pub last_prompt_preview: Option<String>,
    /// Native update timestamp in RFC3339.
    pub last_activity_at: String,
}

/// Bounded native discovery options. Final import and query filtering belongs to the service.
#[derive(Debug, Clone)]
pub struct ListOptions {
    /// Canonical exact cwd filter; none discovers across directories.
    pub cwd: Option<String>,
    /// Maximum distinct sessions inspected, in the range 1..=4096.
    pub scan_limit: usize,
}

/// Complete native facts used to validate an import or refresh before publishing host state.
#[derive(Debug, Clone)]
pub struct SessionHistory {
    /// Immediate native parent for a provider-owned child, when present.
    pub parent_id: Option<String>,
    /// Native identity and display metadata.
    pub descriptor: SessionDescriptor,
    /// Native creation timestamp in RFC3339.
    pub created_at: String,
    /// Native model and effort, constrained by this adapter's execution policy.
    pub config: StoredAgentConfig,
    /// True when the provider reports an active or incomplete turn.
    pub active: bool,
    /// Ordered completed display items.
    pub entries: Vec<NativeItem>,
}
