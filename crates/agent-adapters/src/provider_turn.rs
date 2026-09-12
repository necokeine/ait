//! Structured one-turn conversion. Session/Run ownership stays in the host.
use crate::{
    LLMClient,
    llm::{AssistantContent, Message},
};
use ait_domain::{
    AgentConfiguration, DomainError, ErrorCode, ProjectedMessage, RunUsage, SubMessage, ToolUse,
};
use ait_ports::{AgentInvocation, AgentResponse};
use rig::completion::message::{ToolCall, ToolResultContent, UserContent};
use std::collections::HashMap;

fn invalid() -> DomainError {
    DomainError::invariant(
        ErrorCode::ProviderFailed,
        "provider returned an invalid or oversized message",
    )
}

/// Invoke a single API completion with only executor-supported definitions.
///
/// # Errors
/// Returns bounded diagnostics without exposing HTTP response bodies or arguments.
pub async fn complete_turn(
    client: &LLMClient,
    config: &AgentConfiguration,
    invocation: AgentInvocation,
    executable: &[String],
) -> Result<AgentResponse, DomainError> {
    let mut history = project_path(invocation.message_path)?;
    let current = history.pop().ok_or_else(invalid)?;
    let mut request = client.completion_request_with_history(&config.model, history, current);
    request.tools.retain(|tool| executable.contains(&tool.name));
    for tool in &mut request.tools {
        if let Some(schema) = ait_tools::host::parameters(&tool.name) {
            tool.parameters = schema;
        }
        if tool.name == "bash" {
            tool.description = "Run a controlled command: echo, printf or sleep (0-30 seconds). No shell expansion, redirection, filesystem access, background jobs or escalation. Output is bounded to 64 KiB.".into();
        }
    }
    if let Some(effort) = &config.reasoning_effort {
        client
            .apply_reasoning_effort(&mut request, effort)
            .map_err(|_| invalid())?;
    }
    let response = client.complete(request).await.map_err(|failure| {
        let code = match failure.kind {
            crate::AdapterErrorKind::Cancelled => ErrorCode::RunCancelled,
            crate::AdapterErrorKind::InvalidConfiguration => ErrorCode::InvalidConfiguration,
            _ => ErrorCode::ProviderFailed,
        };
        if failure.retryable {
            DomainError::transient(code, "provider request unavailable or timed out")
        } else {
            DomainError::invariant(code, "provider request rejected or response invalid")
        }
    })?;
    let mut sub_messages = Vec::new();
    for part in response.choice {
        let converted = match part {
            AssistantContent::Text(text) => SubMessage::Text { text: text.text },
            AssistantContent::ToolCall(call) => {
                let call_id = call.wire_call_id().to_owned();
                if call.provider.is_none() || call_id.len() > 256 || call.function.name.len() > 64 {
                    return Err(invalid());
                }
                SubMessage::ToolUse(ToolUse {
                    call_id,
                    tool_name: call.function.name.clone(),
                    arguments: call.function.arguments.to_string(),
                    provider_metadata: Some(serde_json::to_string(&call).map_err(|_| invalid())?),
                })
            }
            part @ AssistantContent::Reasoning(_) => SubMessage::StructuredData {
                media_type: "application/vnd.ait.provider-reasoning+json".into(),
                value: serde_json::to_string(&part).map_err(|_| invalid())?,
            },
            _ => return Err(invalid()),
        };
        sub_messages.push(converted);
    }
    if sub_messages.len() > 32
        || serde_json::to_vec(&sub_messages)
            .map_err(|_| invalid())?
            .len()
            > 262_144
    {
        return Err(invalid());
    }
    Ok(AgentResponse {
        sub_messages,
        usage: RunUsage {
            input_tokens: response.usage.input_tokens,
            output_tokens: response.usage.output_tokens,
            cached_input_tokens: response.usage.cached_input_tokens,
            ..RunUsage::default()
        },
    })
}

fn project_path(path: Vec<ProjectedMessage>) -> Result<Vec<Message>, DomainError> {
    let mut history = Vec::new();
    let mut calls = HashMap::<String, ToolCall>::new();
    for entry in path {
        let ProjectedMessage::Visible(message) = entry else {
            return Err(invalid());
        };
        if let Some(result) = message.tool_result {
            let call = calls.get(&result.call_id).ok_or_else(invalid)?;
            let output = serde_json::to_string(&result).map_err(|_| invalid())?;
            history.push(Message::User {
                content: vec![UserContent::tool_result_for(
                    call.id.clone(),
                    call.provider.clone(),
                    &call.function.name,
                    vec![ToolResultContent::text(output)],
                )],
            });
            continue;
        }
        if message.role == ait_domain::MessageRole::Assistant {
            let mut content = Vec::new();
            for part in message.sub_messages {
                match part {
                    SubMessage::Text { text } => content.push(AssistantContent::text(text)),
                    SubMessage::ToolUse(tool) => {
                        let call: ToolCall = serde_json::from_str(
                            tool.provider_metadata.as_deref().ok_or_else(invalid)?,
                        )
                        .map_err(|_| invalid())?;
                        calls.insert(tool.call_id, call.clone());
                        content.push(AssistantContent::ToolCall(call));
                    }
                    SubMessage::StructuredData { media_type, value }
                        if media_type == "application/vnd.ait.provider-reasoning+json" =>
                    {
                        content.push(serde_json::from_str(&value).map_err(|_| invalid())?);
                    }
                    _ => return Err(invalid()),
                }
            }
            history.push(Message::Assistant { id: None, content });
        } else {
            let text: String = message
                .sub_messages
                .into_iter()
                .filter_map(|part| match part {
                    SubMessage::Text { text } => Some(text),
                    _ => None,
                })
                .collect();
            history.push(if message.role == ait_domain::MessageRole::System {
                Message::system(text)
            } else {
                Message::user(text)
            });
        }
    }
    Ok(history)
}
