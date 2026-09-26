//! Configurable next-turn settings, matching Paseo's nullable patch semantics.

use serde::Deserialize;
use server_domain::agent_runtime::StoredAgentConfig;

/// Configuration methods supported by the native worker.
pub const CAPABILITIES: &[&str] = &[
    "agent.model.set.request",
    "agent.thinking.set.request",
    "agent.config.apply.request",
];

/// Three-state patch value, distinguishing omission from explicit null.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum NullableSetting<T = String> {
    /// The request omitted the field.
    #[default]
    Unchanged,
    /// Explicit null removes the host override.
    Clear,
    /// Replace the override with a selected value.
    Set(T),
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for NullableSetting<T> {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Option::<T>::deserialize(deserializer).map(|value| value.map_or(Self::Clear, Self::Set))
    }
}

impl<T: Clone> NullableSetting<T> {
    fn apply(&self, current: &mut Option<T>) {
        match self {
            Self::Unchanged => {}
            Self::Clear => *current = None,
            Self::Set(value) => *current = Some(value.clone()),
        }
    }
}

/// An omitted value preserves the setting; an explicit null restores provider inheritance.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConfigPatch {
    /// Explicit workflow mode; null is rejected by the service.
    #[serde(default)]
    pub mode_id: NullableSetting,
    /// Feature values merged into existing selections, validated by the native adapter.
    pub feature_values: Option<std::collections::BTreeMap<String, serde_json::Value>>,
    /// Selected model, or explicit null for the provider's inherited model.
    #[serde(default)]
    pub model_id: NullableSetting,
    /// Selected thinking option, or explicit null for provider inheritance.
    #[serde(default)]
    pub thinking_option_id: NullableSetting,
    /// Replace native provider options; null restores native defaults.
    #[serde(default)]
    pub provider_options: NullableSetting<std::collections::BTreeMap<String, serde_json::Value>>,
    /// Replace configured MCP servers; null removes host-provided servers.
    #[serde(default)]
    pub mcp_servers: NullableSetting<std::collections::BTreeMap<String, serde_json::Value>>,
    /// Replace exact MCP preapprovals; null removes host-provided grants.
    #[serde(default)]
    pub tool_policy: NullableSetting<serde_json::Value>,
    /// Replace the appended system prompt; null restores the provider prompt.
    #[serde(default)]
    pub system_prompt: NullableSetting,
}

impl ConfigPatch {
    /// Apply only present fields to a copy of `current`, preserving unrelated configuration.
    #[must_use]
    pub fn apply(&self, current: &StoredAgentConfig) -> StoredAgentConfig {
        let mut next = current.clone();
        self.mode_id.apply(&mut next.mode_id);
        if let Some(features) = &self.feature_values {
            next.feature_values
                .get_or_insert_with(std::collections::BTreeMap::new)
                .extend(features.clone());
        }
        self.model_id.apply(&mut next.model);
        self.thinking_option_id.apply(&mut next.thinking_option_id);
        self.provider_options.apply(&mut next.provider_options);
        self.mcp_servers.apply(&mut next.mcp_servers);
        self.tool_policy.apply(&mut next.tool_policy);
        self.system_prompt.apply(&mut next.system_prompt);
        next
    }
}

#[cfg(test)]
mod tests;
