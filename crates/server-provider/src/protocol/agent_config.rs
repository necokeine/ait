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
pub enum NullableSetting {
    /// The request omitted the field.
    #[default]
    Unchanged,
    /// Explicit null removes the host override.
    Clear,
    /// Replace the override with a selected value.
    Set(String),
}

impl<'de> Deserialize<'de> for NullableSetting {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Option::<String>::deserialize(deserializer)
            .map(|value| value.map_or(Self::Clear, Self::Set))
    }
}

impl NullableSetting {
    fn apply(&self, current: &mut Option<String>) {
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
    /// Selected model, or explicit null for the provider's inherited model.
    #[serde(default)]
    pub model_id: NullableSetting,
    /// Selected thinking option, or explicit null for provider inheritance.
    #[serde(default)]
    pub thinking_option_id: NullableSetting,
}

impl ConfigPatch {
    /// Apply only present fields to a copy of `current`, preserving unrelated configuration.
    #[must_use]
    pub fn apply(&self, current: &StoredAgentConfig) -> StoredAgentConfig {
        let mut next = current.clone();
        self.model_id.apply(&mut next.model);
        self.thinking_option_id.apply(&mut next.thinking_option_id);
        next
    }
}

#[cfg(test)]
mod tests;
