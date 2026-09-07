//! Shared provider connections and reusable or Session-owned Agent configuration.
#![allow(missing_docs)]

use serde::{Deserialize, Serialize};

/// Adapter selection, independent of any transport or provider SDK.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    Codex,
    #[serde(rename = "openai")]
    OpenAI,
    #[serde(rename = "deepseek")]
    DeepSeek,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderModel {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub reasoning_efforts: Vec<String>,
}

/// Public connection metadata. Credentials are held separately by an adapter.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentProvider {
    pub id: String,
    pub name: String,
    pub kind: ProviderKind,
    pub url: Option<String>,
    pub models: Vec<ProviderModel>,
}

/// A mutable Agent's configuration, copied into each Run before execution.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentConfiguration {
    pub provider_id: String,
    pub model: String,
    #[serde(default)]
    pub reasoning_effort: Option<String>,
}
