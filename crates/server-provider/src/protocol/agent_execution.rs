//! Native Agent execution requests using Paseo's canonical method and field names.

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
    /// Provider identity: Codex or Claude Code.
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
    /// Durable key for retries and creation observers.
    pub idempotency_key: Option<String>,
    /// Attach progress observation to this connection.
    pub subscribe: Option<bool>,
    /// Optional first text turn, submitted after native registration commits.
    pub initial_prompt: Option<String>,
    /// Initial inline raster images.
    #[serde(default)]
    pub images: Vec<super::prompt::PromptImage>,
    /// Initial contextual attachments.
    #[serde(default)]
    pub attachments: Vec<serde_json::Value>,
    /// Optional identity for the initial user input.
    pub client_message_id: Option<String>,
    /// Optional native structured output constraint.
    pub output_schema: Option<serde_json::Value>,
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

/// Explicit delivery policy for an already active native turn.
#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ActiveTurnBehavior {
    /// Interrupt the active turn and deliver this input after its terminal acknowledgement.
    #[default]
    Interrupt,
    /// Ask the provider to admit text into the current turn, without interrupting it.
    Steer,
}

/// Submit text, optionally steering the active native turn.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SendRequest {
    /// Full ID, unambiguous prefix, or exact title.
    pub agent_id: String,
    /// Nonempty text, at most 64 KiB.
    pub text: String,
    /// Omission interrupts ordinary foreground work; voice-owned turns remain exclusive.
    pub active_turn_behavior: Option<ActiveTurnBehavior>,
    /// Client retry identity (Paseo's `messageId`).
    pub message_id: Option<String>,
    /// Inline raster images.
    #[serde(default)]
    pub images: Vec<super::prompt::PromptImage>,
    /// Contextual attachments.
    #[serde(default)]
    pub attachments: Vec<serde_json::Value>,
    /// Optional structured output constraint.
    pub output_schema: Option<serde_json::Value>,
}

impl SendRequest {
    /// Consume the request's rich input after the caller resolves its Agent and delivery policy.
    #[must_use]
    pub fn into_prompt(self) -> super::prompt::AgentPrompt {
        super::prompt::AgentPrompt {
            text: self.text,
            images: self.images,
            attachments: self.attachments,
            client_message_id: self.message_id,
            output_schema: self.output_schema,
        }
    }
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
