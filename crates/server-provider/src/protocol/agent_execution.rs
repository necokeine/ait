//! Text-only Agent execution requests using Paseo's canonical method and field names.

use std::collections::BTreeMap;

use serde::Deserialize;
use server_domain::agent_runtime::{AgentPersistenceHandle, StoredAgentConfig};

/// Methods backed by the native Provider worker.
pub const CAPABILITIES: &[&str] = &[
    "agent.create.request",
    "agent.resume.request",
    "agent.message.send.request",
    "agent.cancel.request",
    "agent.finish.wait.request",
];

/// Native session configuration. Advanced fields are validated before any side effect.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionConfig {
    /// Provider identity; this phase implements Codex.
    pub provider: String,
    /// Absolute existing directory.
    pub cwd: String,
    /// Optional display title.
    pub title: Option<String>,
    /// Persistable native configuration.
    #[serde(flatten)]
    pub stored: StoredAgentConfig,
}

/// Create an independently owned native session.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateRequest {
    /// Optional caller-selected UUID.
    pub agent_id: Option<String>,
    /// Native configuration.
    pub config: SessionConfig,
    /// Existing active Workspace with matching directory.
    pub workspace_id: Option<String>,
    /// Initial public labels.
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
}

/// Restore a previously registered native identity without creating a new history.
#[derive(Debug, Clone, Deserialize)]
pub struct ResumeRequest {
    /// Native identity already registered in this server.
    pub handle: AgentPersistenceHandle,
}

/// Submit a single text turn. Additional work is rejected while a turn is active.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SendRequest {
    /// Full ID, unambiguous prefix, or exact title.
    pub agent_id: String,
    /// Nonempty text, at most 64 KiB.
    pub text: String,
}

/// Await native completion without occupying the Provider command lane.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WaitRequest {
    /// Full ID, unambiguous prefix, or exact title.
    pub agent_id: String,
    /// Positive timeout, capped at 30 seconds by this server.
    pub timeout_ms: Option<u64>,
}
