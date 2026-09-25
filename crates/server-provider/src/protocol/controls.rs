//! Remaining Agent controls and Provider inspection requests.

use serde::Deserialize;
use serde_json::Value;

use super::agent_execution::SessionConfig;
use super::timeline::{Cursor, Direction};

/// Methods owned by the serialized Provider worker.
pub const CAPABILITIES: &[&str] = &[
    "agent.rewind.request",
    "agent.commands.list.request",
    "agent.mode.set.request",
    "agent.feature.set.request",
    "agent.permission.resolve.request",
    "agent.provider_subagents.list.request",
    "agent.provider_subagents.timeline.get.request",
    "provider.diagnostic.request",
    "provider.usage.list.request",
];

/// A non-destructive native conversation rewind target.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RewindRequest {
    /// Registered Agent identifier.
    pub agent_id: String,
    /// Native user message to remove together with subsequent turns.
    pub message_id: String,
    /// Only conversation is supported by Codex; files and both are explicitly rejected.
    pub mode: String,
}

/// Discover commands for an existing Agent or an unregistered draft.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CommandsRequest {
    /// Existing Agent identifier, or a UI draft identifier.
    pub agent_id: String,
    /// Native working directory and configuration when the Agent does not exist.
    pub draft_config: Option<SessionConfig>,
}

/// Resolve a pending native approval scoped to one live Agent session.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PermissionRequest {
    /// Registered Agent identifier.
    pub agent_id: String,
    /// Permission identity from the pending request, distinct from the RPC envelope ID.
    pub request_id: String,
    /// Allow/deny decision, with optional question answers.
    pub response: Value,
}

/// Query a provider-owned descendant without registering another host Agent.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SubagentRequest {
    /// Registered root Agent identifier.
    pub parent_agent_id: String,
    /// Native descendant ID for timeline requests.
    pub subagent_id: Option<String>,
    /// Pagination direction.
    pub direction: Option<Direction>,
    /// Exclusive generation/sequence boundary.
    pub cursor: Option<Cursor>,
    /// Page size, with zero meaning the bounded full window.
    pub limit: Option<usize>,
}
