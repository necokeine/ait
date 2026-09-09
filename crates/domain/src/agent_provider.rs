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
    /// Deterministic local provider compiled only into development builds.
    #[cfg(all(feature = "dev-mock-provider", debug_assertions))]
    Mock,
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

#[cfg(test)]
mod tests {
    use super::ProviderKind;

    #[cfg(not(all(feature = "dev-mock-provider", debug_assertions)))]
    #[test]
    fn production_contract_cannot_deserialize_mock_provider_kind() {
        assert!(serde_json::from_str::<ProviderKind>(r#""mock""#).is_err());
    }

    #[cfg(all(feature = "dev-mock-provider", debug_assertions))]
    #[test]
    fn development_contract_round_trips_mock_provider_kind() {
        let kind: ProviderKind = serde_json::from_str(r#""mock""#).unwrap();
        assert_eq!(kind, ProviderKind::Mock);
        assert_eq!(serde_json::to_string(&kind).unwrap(), r#""mock""#);
    }
}
