//! Shared provider connections and reusable or Session-owned Agent configuration.

use serde::{Deserialize, Serialize};

/// Adapter selection, independent of any transport or provider SDK.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    /// Codex app-server provider.
    Codex,
    #[serde(rename = "openai")]
    /// OpenAI-compatible provider.
    OpenAI,
    #[serde(rename = "deepseek")]
    /// `DeepSeek` provider.
    DeepSeek,
    /// Google Gemini provider.
    Gemini,
    #[serde(rename = "minimax")]
    /// `MiniMax` provider.
    MiniMax,
    /// Deterministic local provider compiled only into development builds.
    #[cfg(all(feature = "dev-mock-provider", debug_assertions))]
    Mock,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
/// Model advertised by an agent provider.
pub struct ProviderModel {
    /// Provider-specific model identifier.
    pub id: String,
    /// Human-readable model name.
    pub name: String,
    #[serde(default)]
    /// Reasoning-effort values accepted by the model.
    pub reasoning_efforts: Vec<String>,
}

/// Public connection metadata. Credentials are held separately by an adapter.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentProvider {
    /// Stable provider identifier.
    pub id: String,
    /// Human-readable provider name.
    pub name: String,
    /// Adapter family used to connect to the provider.
    pub kind: ProviderKind,
    /// Optional provider endpoint override.
    pub url: Option<String>,
    /// Models available through this provider.
    pub models: Vec<ProviderModel>,
}

/// A mutable Agent's configuration, copied into each Run before execution.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentConfiguration {
    /// Identifier of the selected provider.
    pub provider_id: String,
    /// Identifier of the selected model.
    pub model: String,
    #[serde(default)]
    /// Optional reasoning-effort setting.
    pub reasoning_effort: Option<String>,
    /// Reserved Agent-authored instructions. Persisted and snapshotted only;
    /// current adapters deliberately do not add this value to provider prompts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
}

#[cfg(test)]
mod tests;
