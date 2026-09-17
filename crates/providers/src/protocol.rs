use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
/// Author role associated with a provider message.
pub enum Role {
    /// System-authored instructions.
    System,
    /// User-authored input or tool results.
    User,
    /// Assistant-authored output or tool calls.
    Assistant,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
/// One typed part of a provider message.
pub enum ContentPart {
    /// Plain text content.
    Text {
        /// Text carried by the message part.
        text: String,
    },
    /// A tool invocation requested by the assistant.
    ToolUse {
        /// Provider-issued identifier for the tool call.
        call_id: String,
        /// Registered tool name.
        name: String,
        /// Structured tool arguments.
        arguments: Value,
    },
    /// Result returned for a previous tool invocation.
    ToolResult {
        /// Identifier of the tool call being completed.
        call_id: String,
        /// Terminal tool execution status.
        status: ToolResultStatus,
        /// Structured tool output, when available.
        output: Option<Value>,
        /// Human-readable error, when execution failed.
        error: Option<String>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
/// Terminal status of a tool invocation.
pub enum ToolResultStatus {
    /// The tool completed successfully.
    Succeeded,
    /// The tool execution failed.
    Failed,
    /// Policy or the user denied execution.
    Denied,
    /// Execution was cancelled.
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
/// Provider-neutral message containing typed content parts.
pub struct ProviderMessage {
    /// Author role for the message.
    pub role: Role,
    /// Ordered content parts in the message.
    pub content: Vec<ContentPart>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
/// Tool metadata and JSON Schema exposed to a provider.
pub struct ToolDefinition {
    /// Registered tool name.
    pub name: String,
    /// Human-readable tool description.
    pub description: String,
    /// JSON Schema accepted as tool input.
    pub input_schema: Value,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
/// Provider-neutral generation parameters.
pub struct ProviderParameters {
    /// Optional upper bound on generated tokens.
    pub max_output_tokens: Option<u32>,
    /// Optional sampling temperature.
    pub temperature: Option<f32>,
    #[serde(default)]
    /// Provider-specific parameters not modeled by the shared contract.
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[allow(clippy::struct_excessive_bools)]
/// Feature set supported or required by a provider request.
pub struct ProviderCapabilities {
    /// Whether incremental response streaming is supported.
    pub streaming: bool,
    /// Whether tools can be invoked.
    pub tool_calling: bool,
    /// Whether multiple tool calls may be active concurrently.
    pub parallel_tool_calls: bool,
    /// Whether token usage is reported.
    pub usage: bool,
    /// Whether system messages are accepted.
    pub system_messages: bool,
}

impl ProviderCapabilities {
    #[must_use]
    /// Lists capabilities required by `self` but absent from `available`.
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
/// Complete provider-neutral request passed to an adapter.
pub struct ProviderRequest {
    /// Ordered conversation messages.
    pub messages: Vec<ProviderMessage>,
    #[serde(default)]
    /// Tool definitions available to the provider.
    pub tools: Vec<ToolDefinition>,
    #[serde(default)]
    /// Generation parameters for the request.
    pub parameters: ProviderParameters,
    #[serde(default)]
    /// Capabilities required to execute the request safely.
    pub required_capabilities: ProviderCapabilities,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
/// Token usage reported by a provider.
pub struct Usage {
    /// Tokens consumed by the request input.
    pub input_tokens: u64,
    /// Tokens generated in the response.
    pub output_tokens: u64,
    /// Total billable tokens reported by the provider.
    pub total_tokens: u64,
    /// Input tokens served from a provider cache, when reported.
    pub cached_input_tokens: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
/// Normalized reason why a provider stream stopped.
pub enum StopReason {
    /// The model completed its response normally.
    Completed,
    /// The model paused to request a tool invocation.
    ToolUse,
    /// The configured output-token limit was reached.
    MaxTokens,
    /// Provider content filtering stopped generation.
    ContentFilter,
    /// The caller cancelled the invocation.
    Cancelled,
    /// Provider-specific stop reason not covered by the shared variants.
    Other(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
/// One normalized event emitted by a streaming provider.
pub enum ProviderEvent {
    /// Incremental assistant text.
    TextDelta {
        /// Newly generated text.
        text: String,
    },
    /// Start of a streamed tool invocation.
    ToolUseStart {
        /// Provider stream index for the tool call.
        index: u32,
        /// Provider-issued tool call identifier.
        call_id: String,
        /// Registered tool name.
        name: String,
    },
    /// Incremental JSON arguments for a tool invocation.
    ToolUseArgumentsDelta {
        /// Provider stream index for the tool call.
        index: u32,
        /// Newly generated argument fragment.
        delta: String,
    },
    /// End of a streamed tool invocation.
    ToolUseEnd {
        /// Provider stream index for the completed tool call.
        index: u32,
    },
    /// Updated token usage.
    Usage {
        /// Provider-reported usage totals.
        usage: Usage,
    },
    /// Terminal stream event.
    Stop {
        /// Normalized terminal reason.
        reason: StopReason,
        /// Original provider stop reason, when available.
        provider_reason: Option<String>,
    },
}
