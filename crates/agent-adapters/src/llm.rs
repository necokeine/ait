//! Stateless, single-request LLM access through Rig.
//!
//! Rig types belong to this adapter boundary, never to domain or store ports.
//! A completed request does not complete a host Run or execute returned tools.

use std::{fmt, time::Duration};

use ait_tools::ToolSetRegistry;
use rig::{
    client::{CompletionClient, ModelListingClient},
    completion::{CompletionError, CompletionModel},
    model::ModelListingError,
    providers::{deepseek, openai},
};

pub use rig::{
    completion::{AssistantContent, CompletionRequest, CompletionResponse, Message},
    model::{Model, ModelList},
};

use crate::{AdapterError, AdapterErrorKind};

mod deepseek_http;
use deepseek_http::DeepSeekHttp;

/// Provider selected explicitly when constructing a client.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LLMProvider {
    /// `OpenAI` Responses API.
    OpenAI,
    /// `DeepSeek` Chat Completions API.
    DeepSeek,
}

/// In-memory connection options. Deliberately not serializable.
#[derive(Clone)]
pub struct LLMClientConfig {
    /// Provider dialect and default API origin.
    pub provider: LLMProvider,
    api_key: String,
    /// Optional API root, including any version prefix (for example `/v1`).
    pub base_url: Option<String>,
    /// Total HTTP request timeout, including reading the response body.
    pub timeout: Duration,
    /// Shared default prompt/tools and exact provider/model overrides.
    pub tool_sets: ToolSetRegistry,
}

impl LLMClientConfig {
    /// Creates options with the provider's official URL and a 120-second timeout.
    #[must_use]
    pub fn new(provider: LLMProvider, api_key: impl Into<String>) -> Self {
        Self {
            provider,
            api_key: api_key.into(),
            base_url: None,
            timeout: Duration::from_mins(2),
            tool_sets: ToolSetRegistry::default(),
        }
    }
}

impl fmt::Debug for LLMClientConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LLMClientConfig")
            .field("provider", &self.provider)
            .field("api_key", &"[REDACTED]")
            .field("base_url", &self.base_url.as_ref().map(|_| "[CONFIGURED]"))
            .field("timeout", &self.timeout)
            .field("tool_sets", &"[CONFIGURED]")
            .finish()
    }
}

#[derive(Clone)]
enum RigClient {
    OpenAI(openai::Client),
    DeepSeek(deepseek::Client<DeepSeekHttp>),
}

/// A reusable Rig client for either `OpenAI` or `DeepSeek`.
///
/// Each operation issues one request with no agent loop or automatic retries.
/// Dropping its future cancels the in-flight operation.
///
/// ```no_run
/// use ait_agent_adapters::{LLMClient, LLMClientConfig, LLMProvider};
///
/// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
/// let key = std::env::var("DEEPSEEK_API_KEY")?;
/// let client = LLMClient::new(LLMClientConfig::new(LLMProvider::DeepSeek, key))?;
/// let models = client.list_models().await?;
/// let model = std::env::var("LLM_MODEL")?;
/// let text = client.prompt(&model, "Say hello.").await?;
///
/// let mut request = client.completion_request(&model, "Explain ownership in Rust.");
/// request.max_tokens = Some(256);
/// let response = client.complete(request).await?;
/// # Ok(())
/// # }
/// ```
#[derive(Clone)]
pub struct LLMClient {
    inner: RigClient,
    tool_sets: ToolSetRegistry,
}

impl fmt::Debug for LLMClient {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LLMClient")
            .field("provider", &self.provider())
            .finish_non_exhaustive()
    }
}

impl LLMClient {
    /// Validates options and constructs the selected Rig client without network I/O.
    ///
    /// # Errors
    /// Returns `InvalidConfiguration` for invalid credentials, URLs or timeouts.
    pub fn new(config: LLMClientConfig) -> Result<Self, AdapterError> {
        if config.api_key.trim().is_empty()
            || config.api_key.chars().any(char::is_whitespace)
            || config.api_key.chars().any(char::is_control)
        {
            return Err(invalid(
                "API key must be non-empty and contain no whitespace or controls",
            ));
        }
        if config.timeout.is_zero() {
            return Err(invalid("request timeout must be greater than zero"));
        }
        let base_url = config.base_url.as_deref().unwrap_or(match config.provider {
            LLMProvider::OpenAI => "https://api.openai.com/v1",
            LLMProvider::DeepSeek => "https://api.deepseek.com",
        });
        let url = reqwest::Url::parse(base_url)
            .map_err(|_| invalid("base URL must be an absolute HTTP(S) API root"))?;
        if !matches!(url.scheme(), "http" | "https")
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err(invalid(
                "base URL must be HTTP(S), without credentials, query or fragment",
            ));
        }
        let http = reqwest::Client::builder()
            .timeout(config.timeout)
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .build()
            .map_err(|_| invalid("could not build LLM HTTP client"))?;
        let base_url = url.as_str().trim_end_matches('/');
        let inner = match config.provider {
            LLMProvider::OpenAI => RigClient::OpenAI(
                openai::Client::builder()
                    .api_key(config.api_key)
                    .base_url(base_url)
                    .http_client(http)
                    .build()
                    .map_err(|_| invalid("could not build OpenAI client"))?,
            ),
            LLMProvider::DeepSeek => RigClient::DeepSeek(
                deepseek::Client::builder()
                    .api_key(config.api_key)
                    .base_url(base_url)
                    .http_client(DeepSeekHttp(http))
                    .build()
                    .map_err(|_| invalid("could not build DeepSeek client"))?,
            ),
        };
        Ok(Self {
            inner,
            tool_sets: config.tool_sets,
        })
    }

    /// Returns the provider selected at construction.
    #[must_use]
    pub fn provider(&self) -> LLMProvider {
        match &self.inner {
            RigClient::OpenAI(_) => LLMProvider::OpenAI,
            RigClient::DeepSeek(_) => LLMProvider::DeepSeek,
        }
    }

    /// Fetches the models visible to the configured API key using Rig's lister.
    ///
    /// The list may include models for other API capabilities, such as embeddings.
    /// # Errors
    /// Returns a redacted authentication, HTTP, transport or protocol error.
    pub async fn list_models(&self) -> Result<ModelList, AdapterError> {
        match &self.inner {
            RigClient::OpenAI(client) => client.list_models().await,
            RigClient::DeepSeek(client) => client.list_models().await,
        }
        .map_err(|error| match error {
            ModelListingError::ApiError { status_code, .. } => http_error(status_code),
            ModelListingError::AuthError { .. } => http_error(401),
            ModelListingError::RequestError { .. } => transport_error(),
            ModelListingError::ParseError { .. } => {
                AdapterError::protocol("invalid model list response")
            }
        })
    }

    /// Builds a request with the selected system prompt first, followed by the
    /// supplied message, and a separate default function catalog. Nothing runs.
    /// Callers may edit the request before passing it to `complete`; a tool loop
    /// must provide executors for the tools it exposes. Use `text_request` when
    /// the caller can consume only text.
    #[must_use]
    pub fn completion_request(&self, model: &str, prompt: impl Into<Message>) -> CompletionRequest {
        self.completion_request_with_history(model, Vec::new(), prompt)
    }

    /// Assembles system instructions, existing history, then the current message.
    /// History is moved verbatim and user text is never interpolated into the
    /// system prompt. Existing project system snapshots remain in history.
    #[must_use]
    pub fn completion_request_with_history(
        &self,
        model: &str,
        history: Vec<Message>,
        prompt: impl Into<Message>,
    ) -> CompletionRequest {
        let prompt = prompt.into();
        let profile = self.tool_sets.resolve(
            match self.provider() {
                LLMProvider::OpenAI => "openai",
                LLMProvider::DeepSeek => "deepseek",
            },
            model,
        );
        let mut request = match &self.inner {
            RigClient::OpenAI(client) => client
                .completion_model(model)
                .completion_request(prompt)
                .model(model)
                .build(),
            RigClient::DeepSeek(client) => client
                .completion_model(model)
                .completion_request(prompt)
                .model(model)
                .build(),
        };
        request.chat_history.splice(
            0..0,
            std::iter::once(Message::system(profile.system_prompt())).chain(history),
        );
        request.tools = profile
            .tools()
            .iter()
            .map(|tool| rig::completion::ToolDefinition {
                name: tool.name.clone(),
                description: tool.description.clone(),
                parameters: tool.parameters.clone(),
            })
            .collect();
        request
    }

    /// Builds a text-only request with the same system prompt and no tools.
    /// Used by hosts that do not yet implement tool execution or rich results.
    #[must_use]
    pub fn text_request(&self, model: &str, prompt: impl Into<Message>) -> CompletionRequest {
        let mut request = self.completion_request(model, prompt);
        request.tools.clear();
        request
    }

    /// Applies one caller-selected reasoning effort to a request.
    ///
    /// `OpenAI` Responses accepts `reasoning.effort`. `DeepSeek` Chat
    /// Completions instead uses `reasoning_effort` while thinking is enabled; its
    /// adapter-owned `off` choice must be expressed as `thinking.type =
    /// "disabled"` and must not cross the wire as a reasoning effort.
    /// Existing provider-specific request parameters are preserved.
    ///
    /// # Errors
    /// Rejects an empty effort or non-object `additional_params` locally.
    pub fn apply_reasoning_effort(
        &self,
        request: &mut CompletionRequest,
        effort: &str,
    ) -> Result<(), AdapterError> {
        if effort.trim().is_empty() {
            return Err(invalid("reasoning effort must be non-empty"));
        }
        if request.additional_params.is_none() {
            request.additional_params = Some(serde_json::json!({}));
        }
        let params = request
            .additional_params
            .as_mut()
            .and_then(serde_json::Value::as_object_mut)
            .ok_or_else(|| invalid("additional completion parameters must be an object"))?;
        match self.provider() {
            LLMProvider::OpenAI => {
                let reasoning = params
                    .entry("reasoning")
                    .or_insert_with(|| serde_json::json!({}));
                if let serde_json::Value::Object(reasoning) = reasoning {
                    reasoning.insert("effort".into(), serde_json::json!(effort));
                } else {
                    *reasoning = serde_json::json!({"effort": effort});
                }
            }
            LLMProvider::DeepSeek => {
                params.remove("reasoning_effort");
                if effort == "off" {
                    params.insert("thinking".into(), serde_json::json!({"type": "disabled"}));
                } else {
                    params.insert("thinking".into(), serde_json::json!({"type": "enabled"}));
                    params.insert("reasoning_effort".into(), serde_json::json!(effort));
                }
            }
        }
        Ok(())
    }

    /// Sends one non-streaming API call and preserves Rig content, usage and metadata.
    /// Returned tool calls are data only; the caller owns their execution.
    ///
    /// # Errors
    /// Rejects a missing model or invalid history locally, and redacts provider errors.
    pub async fn complete(
        &self,
        request: CompletionRequest,
    ) -> Result<CompletionResponse, AdapterError> {
        let model = request
            .model
            .as_deref()
            .filter(|model| !model.trim().is_empty())
            .ok_or_else(|| invalid("completion request requires a model"))?;
        request
            .validate_message_content()
            .map_err(|_| invalid("completion request contains empty history or message content"))?;
        match &self.inner {
            RigClient::OpenAI(client) => client.completion_model(model).completion(request).await,
            RigClient::DeepSeek(client) => client.completion_model(model).completion(request).await,
        }
        .map_err(|error| completion_error(&error))
    }

    /// Sends one text prompt and joins the response's text blocks in order.
    /// Use `complete` to retain reasoning, tool calls, usage and response metadata.
    ///
    /// # Errors
    /// Returns a completion error or a protocol error when no text was returned.
    pub async fn prompt(&self, model: &str, prompt: &str) -> Result<String, AdapterError> {
        let response = self.complete(self.text_request(model, prompt)).await?;
        let mut text = String::new();
        let mut has_text = false;
        for content in response.choice {
            if let AssistantContent::Text(part) = content {
                has_text = true;
                text.push_str(&part.text);
            }
        }
        if !has_text {
            return Err(AdapterError::protocol(
                "completion returned no text content",
            ));
        }
        Ok(text)
    }
}

fn invalid(message: &'static str) -> AdapterError {
    AdapterError::new(AdapterErrorKind::InvalidConfiguration, message, false)
}

fn transport_error() -> AdapterError {
    AdapterError::new(
        AdapterErrorKind::Unavailable,
        "LLM transport failed or timed out",
        true,
    )
}

// SDK error bodies can echo keys, prompts and gateway URLs. Preserve only safe
// classification/status information, never raw SDK Display/Debug or sources.
fn http_error(status: u16) -> AdapterError {
    let (kind, retryable) = match status {
        401 | 403 => (AdapterErrorKind::Authentication, false),
        429 => (AdapterErrorKind::RateLimited, true),
        408 | 500..=599 => (AdapterErrorKind::Unavailable, true),
        _ => (AdapterErrorKind::Protocol, false),
    };
    let mut error = AdapterError::new(kind, format!("LLM API returned HTTP {status}"), retryable);
    error.code = Some(status.to_string());
    error
}

fn completion_error(error: &CompletionError) -> AdapterError {
    if let Some(status) = error.provider_response_status() {
        return http_error(status.as_u16());
    }
    match error {
        CompletionError::HttpError(_) => transport_error(),
        CompletionError::RequestError(_) | CompletionError::UrlError(_) => {
            invalid("invalid LLM completion request")
        }
        _ => AdapterError::protocol("invalid LLM completion response"),
    }
}
