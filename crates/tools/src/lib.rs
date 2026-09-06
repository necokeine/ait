//! Provider-neutral tool definitions and system prompts for API requests.
//!
//! A catalog describes an executor contract; it does not execute tools, grant
//! permissions, or own a Run. Hosts must supply a tool bridge before using it
//! in a tool loop. SDK conversion belongs to `ait-agent-adapters`.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Version of Ait's bundled prompt and tool contracts.
pub const DEFAULT_TOOL_SET_REVISION: &str = "ait-default-v1";
/// Pinned upstream used for the Standard catalog and Minimal editor contract.
pub const DEEPSEEK_HARNESS_REVISION: &str = "d347e703908d0406b7a7ef80e3a0e594d86b2215";
/// Ait system instructions, kept separate from user text and JSON tool schemas.
pub const DEFAULT_SYSTEM_PROMPT: &str = include_str!("../prompts/default.md");

/// One model-facing function, independent of any provider's wire envelope.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolDefinition {
    /// Stable function name used to route a returned tool call.
    pub name: String,
    /// Purpose and execution semantics visible to the model.
    pub description: String,
    /// JSON Schema for an object containing the function's arguments.
    pub parameters: Value,
}

/// Invalid catalog configuration. Errors never echo prompt or schema content.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ToolSetError {
    /// The system prompt must contain instructions.
    #[error("tool set requires a non-empty system prompt")]
    EmptyPrompt,
    /// Function names must be unique portable API identifiers.
    #[error("tool names must be unique ASCII identifiers of 1 to 64 characters")]
    InvalidName,
    /// A definition requires documentation and an object argument schema.
    #[error("tool requires a non-empty description and an object parameter schema")]
    InvalidDefinition,
}

/// An immutable system prompt and deterministically ordered tool catalog.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolSet {
    system_prompt: String,
    tools: Vec<ToolDefinition>,
}

impl ToolSet {
    /// Constructs a catalog, checking names and object roots and sorting by name.
    /// Custom schema semantics remain the caller's responsibility.
    ///
    /// # Errors
    /// Rejects empty prompts/descriptions, duplicate or invalid names, and
    /// parameter schemas without an object root and a properties object.
    pub fn new(
        system_prompt: impl Into<String>,
        mut tools: Vec<ToolDefinition>,
    ) -> Result<Self, ToolSetError> {
        let system_prompt = system_prompt.into();
        if system_prompt.trim().is_empty() {
            return Err(ToolSetError::EmptyPrompt);
        }
        tools.sort_by(|a, b| a.name.cmp(&b.name));
        for (index, tool) in tools.iter().enumerate() {
            if tool.name.is_empty()
                || tool.name.len() > 64
                || !tool
                    .name
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
                || (index > 0 && tools[index - 1].name == tool.name)
            {
                return Err(ToolSetError::InvalidName);
            }
            if tool.description.trim().is_empty()
                || tool.parameters.get("type").and_then(Value::as_str) != Some("object")
                || !tool
                    .parameters
                    .get("properties")
                    .is_some_and(Value::is_object)
            {
                return Err(ToolSetError::InvalidDefinition);
            }
        }
        Ok(Self {
            system_prompt,
            tools,
        })
    }

    /// Instructions to place before conversation history and the current user.
    #[must_use]
    pub fn system_prompt(&self) -> &str {
        &self.system_prompt
    }

    /// Function definitions in stable name order.
    #[must_use]
    pub fn tools(&self) -> &[ToolDefinition] {
        &self.tools
    }

    /// Resolves a returned function name without executing it.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&ToolDefinition> {
        self.tools
            .binary_search_by(|tool| tool.name.as_str().cmp(name))
            .ok()
            .map(|index| &self.tools[index])
    }
}

impl Default for ToolSet {
    fn default() -> Self {
        let mut tools: Vec<ToolDefinition> =
            serde_json::from_str(include_str!("../catalog/default.json"))
                .expect("bundled tool definitions are valid JSON");
        // Standard exposes the shell appropriate to the host, never both.
        tools.retain(|tool| tool.name != if cfg!(windows) { "bash" } else { "pwsh" });
        Self::new(DEFAULT_SYSTEM_PROMPT, tools).expect("bundled tool set is valid")
    }
}

/// Shared default with exact provider/model overrides; no model-name guessing.
#[derive(Clone, Debug, Default)]
pub struct ToolSetRegistry {
    default: ToolSet,
    overrides: BTreeMap<(String, String), ToolSet>,
}

impl ToolSetRegistry {
    /// Uses a caller-supplied default until an exact model override is registered.
    #[must_use]
    pub fn new(default: ToolSet) -> Self {
        Self {
            default,
            overrides: BTreeMap::new(),
        }
    }

    /// Inserts or replaces one exact provider/model profile.
    /// Provider keys used by the built-in API adapter are `deepseek` and `openai`.
    pub fn insert(&mut self, provider: &str, model: &str, tool_set: ToolSet) {
        self.overrides
            .insert((provider.to_owned(), model.to_owned()), tool_set);
    }

    /// Chooses an exact override, falling back to the shared default.
    #[must_use]
    pub fn resolve(&self, provider: &str, model: &str) -> &ToolSet {
        self.overrides
            .get(&(provider.to_owned(), model.to_owned()))
            .unwrap_or(&self.default)
    }
}
