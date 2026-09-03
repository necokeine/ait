use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    System,
    User,
    Assistant,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentPart {
    Text {
        text: String,
    },
    ToolUse {
        call_id: String,
        name: String,
        arguments: Value,
    },
    ToolResult {
        call_id: String,
        status: ToolResultStatus,
        output: Option<Value>,
        error: Option<String>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolResultStatus {
    Succeeded,
    Failed,
    Denied,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProviderMessage {
    pub role: Role,
    pub content: Vec<ContentPart>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct ProviderParameters {
    pub max_output_tokens: Option<u32>,
    pub temperature: Option<f32>,
    #[serde(default)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[allow(clippy::struct_excessive_bools)]
pub struct ProviderCapabilities {
    pub streaming: bool,
    pub tool_calling: bool,
    pub parallel_tool_calls: bool,
    pub usage: bool,
    pub system_messages: bool,
}

impl ProviderCapabilities {
    #[must_use]
    pub fn missing_from(self, available: Self) -> Vec<&'static str> {
        let mut missing = Vec::new();
        for (required, supplied, name) in [
            (self.streaming, available.streaming, "streaming"),
            (self.tool_calling, available.tool_calling, "tool_calling"),
            (
                self.parallel_tool_calls,
                available.parallel_tool_calls,
                "parallel_tool_calls",
            ),
            (self.usage, available.usage, "usage"),
            (
                self.system_messages,
                available.system_messages,
                "system_messages",
            ),
        ] {
            if required && !supplied {
                missing.push(name);
            }
        }
        missing
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProviderRequest {
    pub messages: Vec<ProviderMessage>,
    #[serde(default)]
    pub tools: Vec<ToolDefinition>,
    #[serde(default)]
    pub parameters: ProviderParameters,
    #[serde(default)]
    pub required_capabilities: ProviderCapabilities,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub total_tokens: u64,
    pub cached_input_tokens: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    Completed,
    ToolUse,
    MaxTokens,
    ContentFilter,
    Cancelled,
    Other(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ProviderEvent {
    TextDelta {
        text: String,
    },
    ToolUseStart {
        index: u32,
        call_id: String,
        name: String,
    },
    ToolUseArgumentsDelta {
        index: u32,
        delta: String,
    },
    ToolUseEnd {
        index: u32,
    },
    Usage {
        usage: Usage,
    },
    Stop {
        reason: StopReason,
        provider_reason: Option<String>,
    },
}
