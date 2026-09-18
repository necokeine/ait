//! Shared provider connections available to Agent configurations.

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

/// Model advertised by an Agent provider.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderModel {
    /// Provider-specific model identifier.
    pub id: String,
    /// Human-readable model name.
    pub name: String,
    /// Reasoning-effort values accepted by the model.
    #[serde(default)]
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

#[cfg(test)]
mod tests;
