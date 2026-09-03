use std::collections::BTreeSet;

use async_stream::try_stream;
use async_trait::async_trait;
use futures_util::StreamExt;
use reqwest::{Client, StatusCode, header::RETRY_AFTER};
use serde_json::{Value, json};

use crate::{
    ContentPart, ProviderAdapter, ProviderCapabilities, ProviderError, ProviderErrorKind,
    ProviderEvent, ProviderInvocation, ProviderMessage, ProviderStream, RetryDirective, Role,
    StopReason, ToolResultStatus, Usage, validate_request,
};

/// Remote adapter for the OpenAI-compatible chat-completions streaming shape.
/// `AgentDefinition.endpoint` is the full chat-completions URL.
#[derive(Debug, Clone, Default)]
pub struct OpenAiCompatibleProvider {
    client: Client,
}

impl OpenAiCompatibleProvider {
    #[must_use]
    pub fn new(client: Client) -> Self {
        Self { client }
    }
}

#[async_trait]
impl ProviderAdapter for OpenAiCompatibleProvider {
    fn driver(&self) -> &'static str {
        "openai_compatible"
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            streaming: true,
            tool_calling: true,
            parallel_tool_calls: true,
            usage: true,
            system_messages: true,
        }
    }

    #[allow(clippy::too_many_lines)]
    async fn stream(
        &self,
        invocation: ProviderInvocation,
    ) -> Result<ProviderStream, ProviderError> {
        validate_request(&invocation.request, self.capabilities())?;
        let endpoint = invocation
            .endpoint
            .as_deref()
            .ok_or_else(|| ProviderError::invalid("openai-compatible endpoint is required"))?;
        let credential = invocation.credential.as_ref().ok_or_else(|| {
            ProviderError::new(
                ProviderErrorKind::Authentication,
                "credential_ref did not resolve a credential",
                RetryDirective::Never,
            )
        })?;
        let body = encode_request(&invocation.model, &invocation.request);
        let send = self
            .client
            .post(endpoint)
            .bearer_auth(credential.expose_secret())
            .header("x-request-id", &invocation.request_id)
            .json(&body)
            .send();
        let response = tokio::select! {
            () = invocation.cancellation.cancelled() => return Err(ProviderError::cancelled()),
            result = send => result.map_err(classify_transport_error)?,
        };
        if !response.status().is_success() {
            return Err(classify_http_error(response).await);
        }

        let cancellation = invocation.cancellation;
        let mut chunks = response.bytes_stream();
        let output = try_stream! {
            let mut buffer = String::new();
            let mut open_tools = BTreeSet::new();
            let mut pending_stop = None;
            loop {
                let next = tokio::select! {
                    () = cancellation.cancelled() => Err(ProviderError::cancelled()),
                    value = chunks.next() => Ok(value),
                }?;
                let Some(chunk) = next else { break };
                let chunk = chunk.map_err(classify_transport_error)?;
                let text = std::str::from_utf8(&chunk).map_err(|_| ProviderError::new(
                    ProviderErrorKind::Protocol,
                    "provider stream was not UTF-8",
                    RetryDirective::Never,
                ))?;
                buffer.push_str(text);
                buffer = buffer.replace("\r\n", "\n");
                while let Some(boundary) = buffer.find("\n\n") {
                    let frame = buffer[..boundary].to_owned();
                    buffer.drain(..boundary + 2);
                    let data = frame
                        .lines()
                        .filter_map(|line| line.strip_prefix("data:"))
                        .map(str::trim_start)
                        .collect::<Vec<_>>()
                        .join("\n");
                    if data.is_empty() {
                        continue;
                    }
                    if data == "[DONE]" {
                        if let Some((reason, provider_reason)) = pending_stop.take() {
                            yield ProviderEvent::Stop {
                                reason,
                                provider_reason: Some(provider_reason),
                            };
                        }
                        continue;
                    }
                    let payload: Value = serde_json::from_str(&data).map_err(|error| ProviderError::new(
                        ProviderErrorKind::Protocol,
                        format!("invalid SSE JSON: {error}"),
                        RetryDirective::Never,
                    ))?;
                    if let Some(error) = payload.get("error") {
                        Err(provider_payload_error(error))?;
                    }
                    if let Some(usage) = payload.get("usage").filter(|value| !value.is_null()) {
                        yield ProviderEvent::Usage { usage: decode_usage(usage)? };
                    }
                    let Some(choices) = payload.get("choices").and_then(Value::as_array) else {
                        continue;
                    };
                    for choice in choices {
                        if let Some(delta) = choice.get("delta") {
                            if let Some(text) = delta.get("content").and_then(Value::as_str)
                                && !text.is_empty()
                            {
                                yield ProviderEvent::TextDelta { text: text.to_owned() };
                            }
                            if let Some(calls) = delta.get("tool_calls").and_then(Value::as_array) {
                                for call in calls {
                                    let raw_index = call.get("index").and_then(Value::as_u64).unwrap_or(0);
                                    let index = u32::try_from(raw_index).map_err(|_| ProviderError::new(
                                        ProviderErrorKind::Protocol,
                                        "provider tool-call index exceeds u32",
                                        RetryDirective::Never,
                                    ))?;
                                    let call_id = call.get("id").and_then(Value::as_str).unwrap_or_default();
                                    let name = call.pointer("/function/name").and_then(Value::as_str).unwrap_or_default();
                                    if open_tools.insert(index) {
                                        yield ProviderEvent::ToolUseStart {
                                            index,
                                            call_id: call_id.to_owned(),
                                            name: name.to_owned(),
                                        };
                                    }
                                    if let Some(arguments) = call.pointer("/function/arguments").and_then(Value::as_str)
                                        && !arguments.is_empty()
                                    {
                                        yield ProviderEvent::ToolUseArgumentsDelta {
                                            index,
                                            delta: arguments.to_owned(),
                                        };
                                    }
                                }
                            }
                        }
                        if let Some(reason) = choice.get("finish_reason").and_then(Value::as_str) {
                            for index in std::mem::take(&mut open_tools) {
                                yield ProviderEvent::ToolUseEnd { index };
                            }
                            pending_stop = Some((map_stop_reason(reason), reason.to_owned()));
                        }
                    }
                }
            }
            if let Some((reason, provider_reason)) = pending_stop.take() {
                yield ProviderEvent::Stop {
                    reason,
                    provider_reason: Some(provider_reason),
                };
            }
        };
        Ok(Box::pin(output))
    }
}

fn encode_request(model: &str, request: &crate::ProviderRequest) -> Value {
    let messages = request
        .messages
        .iter()
        .map(encode_message)
        .collect::<Vec<_>>();
    let tools = request
        .tools
        .iter()
        .map(|tool| {
            json!({
                "type": "function",
                "function": {
                    "name": tool.name,
                    "description": tool.description,
                    "parameters": tool.input_schema,
                }
            })
        })
        .collect::<Vec<_>>();
    let mut body = json!({
        "model": model,
        "messages": messages,
        "stream": true,
        "stream_options": {"include_usage": true},
    });
    if !tools.is_empty() {
        body["tools"] = Value::Array(tools);
    }
    if let Some(max_tokens) = request.parameters.max_output_tokens {
        body["max_tokens"] = json!(max_tokens);
    }
    if let Some(temperature) = request.parameters.temperature {
        body["temperature"] = json!(temperature);
    }
    for (key, value) in &request.parameters.extra {
        if body.get(key).is_none() {
            body[key] = value.clone();
        }
    }
    body
}

fn encode_message(message: &ProviderMessage) -> Value {
    if let [
        ContentPart::ToolResult {
            call_id,
            status,
            output,
            error,
        },
    ] = message.content.as_slice()
    {
        let content = match status {
            ToolResultStatus::Succeeded => output.clone().unwrap_or(Value::Null).to_string(),
            _ => error.clone().unwrap_or_else(|| format!("tool {status:?}")),
        };
        return json!({"role": "tool", "tool_call_id": call_id, "content": content});
    }
    let role = match message.role {
        Role::System => "system",
        Role::User => "user",
        Role::Assistant => "assistant",
    };
    let text = message
        .content
        .iter()
        .filter_map(|part| match part {
            ContentPart::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("");
    let calls = message
        .content
        .iter()
        .filter_map(|part| match part {
            ContentPart::ToolUse {
                call_id,
                name,
                arguments,
            } => Some(json!({
                "id": call_id,
                "type": "function",
                "function": {"name": name, "arguments": arguments.to_string()},
            })),
            _ => None,
        })
        .collect::<Vec<_>>();
    let mut value = json!({"role": role, "content": text});
    if !calls.is_empty() {
        value["tool_calls"] = Value::Array(calls);
    }
    value
}

fn decode_usage(value: &Value) -> Result<Usage, ProviderError> {
    let input_tokens = value
        .get("prompt_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let output_tokens = value
        .get("completion_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let total_tokens = value
        .get("total_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(input_tokens + output_tokens);
    let cached_input_tokens = value
        .pointer("/prompt_tokens_details/cached_tokens")
        .and_then(Value::as_u64);
    if total_tokens < input_tokens + output_tokens {
        return Err(ProviderError::new(
            ProviderErrorKind::Protocol,
            "provider usage total is inconsistent",
            RetryDirective::Never,
        ));
    }
    Ok(Usage {
        input_tokens,
        output_tokens,
        total_tokens,
        cached_input_tokens,
    })
}

fn map_stop_reason(reason: &str) -> StopReason {
    match reason {
        "stop" => StopReason::Completed,
        "tool_calls" | "function_call" => StopReason::ToolUse,
        "length" => StopReason::MaxTokens,
        "content_filter" => StopReason::ContentFilter,
        other => StopReason::Other(other.to_owned()),
    }
}

#[allow(clippy::needless_pass_by_value)]
fn classify_transport_error(error: reqwest::Error) -> ProviderError {
    let (kind, retry) = if error.is_timeout() {
        (ProviderErrorKind::Timeout, RetryDirective::Backoff)
    } else if error.is_connect() {
        (ProviderErrorKind::Unavailable, RetryDirective::Backoff)
    } else {
        (ProviderErrorKind::Internal, RetryDirective::Backoff)
    };
    ProviderError::new(kind, error.to_string(), retry)
}

async fn classify_http_error(response: reqwest::Response) -> ProviderError {
    let status = response.status();
    let retry_after = response
        .headers()
        .get(RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .map(|seconds| RetryDirective::AfterMillis(seconds.saturating_mul(1000)));
    let body = response.text().await.unwrap_or_default();
    let payload: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
    let message = payload
        .pointer("/error/message")
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            if body.is_empty() {
                "provider request failed"
            } else {
                &body
            }
        })
        .to_owned();
    let provider_code = payload
        .pointer("/error/code")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let (kind, retry) = match status {
        StatusCode::UNAUTHORIZED => (ProviderErrorKind::Authentication, RetryDirective::Never),
        StatusCode::FORBIDDEN => (ProviderErrorKind::Permission, RetryDirective::Never),
        StatusCode::REQUEST_TIMEOUT => (ProviderErrorKind::Timeout, RetryDirective::Backoff),
        StatusCode::TOO_MANY_REQUESTS => (
            ProviderErrorKind::RateLimited,
            retry_after.unwrap_or(RetryDirective::Backoff),
        ),
        status if status.is_server_error() => {
            (ProviderErrorKind::Unavailable, RetryDirective::Backoff)
        }
        _ => (ProviderErrorKind::InvalidRequest, RetryDirective::Never),
    };
    ProviderError {
        kind,
        message,
        retry,
        http_status: Some(status.as_u16()),
        provider_code,
    }
}

fn provider_payload_error(value: &Value) -> ProviderError {
    ProviderError {
        kind: ProviderErrorKind::Protocol,
        message: value
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("provider emitted a stream error")
            .to_owned(),
        retry: RetryDirective::Never,
        http_status: None,
        provider_code: value.get("code").and_then(Value::as_str).map(str::to_owned),
    }
}
