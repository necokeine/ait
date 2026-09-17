use std::{pin::Pin, sync::Arc};

use async_trait::async_trait;
use futures_core::Stream;
use tokio_util::sync::CancellationToken;

use crate::{
    ContentPart, CredentialResolver, ProviderCapabilities, ProviderError, ProviderEvent,
    ProviderRequest, Role, SecretValue,
};

/// Asynchronous stream of normalized provider events.
pub type ProviderStream =
    Pin<Box<dyn Stream<Item = Result<ProviderEvent, ProviderError>> + Send + 'static>>;

#[derive(Clone)]
/// Fully resolved input passed to one provider adapter invocation.
pub struct ProviderInvocation {
    /// Correlation identifier for the invocation.
    pub request_id: String,
    /// Provider model identifier.
    pub model: String,
    /// Optional provider endpoint override.
    pub endpoint: Option<String>,
    /// Resolved credential scoped to this invocation.
    pub credential: Option<SecretValue>,
    /// Provider-neutral request payload.
    pub request: ProviderRequest,
    /// Token used to cancel the in-flight invocation.
    pub cancellation: CancellationToken,
}

impl std::fmt::Debug for ProviderInvocation {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ProviderInvocation")
            .field("request_id", &self.request_id)
            .field("model", &self.model)
            .field("endpoint", &self.endpoint)
            .field("credential", &self.credential)
            .field("request", &self.request)
            .finish_non_exhaustive()
    }
}

#[async_trait]
/// Adapter boundary implemented by each supported provider driver.
pub trait ProviderAdapter: Send + Sync {
    /// Returns the stable driver identifier.
    fn driver(&self) -> &'static str;
    /// Returns capabilities supported by this adapter.
    fn capabilities(&self) -> ProviderCapabilities;

    /// Starts an invocation and returns its normalized event stream.
    ///
    /// # Errors
    ///
    /// Returns a provider-neutral error when validation or stream creation fails.
    async fn stream(&self, invocation: ProviderInvocation)
    -> Result<ProviderStream, ProviderError>;
}

/// Validates provider-neutral request invariants and capability requirements.
///
/// # Errors
///
/// Returns [`ProviderError`] before invoking a provider when capabilities are
/// missing or the message/tool-result shape violates the shared contract.
pub fn validate_request(
    request: &ProviderRequest,
    available: ProviderCapabilities,
) -> Result<(), ProviderError> {
    let missing = request.required_capabilities.missing_from(available);
    if !missing.is_empty() {
        return Err(ProviderError::unsupported(format!(
            "provider lacks required capabilities: {}",
            missing.join(", ")
        )));
    }
    if !request.tools.is_empty() && !available.tool_calling {
        return Err(ProviderError::unsupported(
            "request includes tools but provider does not support tool calling",
        ));
    }
    if request.messages.is_empty() {
        return Err(ProviderError::invalid("message path cannot be empty"));
    }
    for (message_index, message) in request.messages.iter().enumerate() {
        if message.content.is_empty() {
            return Err(ProviderError::invalid(format!(
                "message {message_index} has no content"
            )));
        }
        let mut tool_results = 0;
        for part in &message.content {
            match (message.role, part) {
                (Role::Assistant | Role::System, ContentPart::ToolResult { .. }) => {
                    return Err(ProviderError::invalid(
                        "ToolResult must be carried by a user message",
                    ));
                }
                (Role::System | Role::User, ContentPart::ToolUse { .. }) => {
                    return Err(ProviderError::invalid(
                        "ToolUse must be carried by an assistant message",
                    ));
                }
                (_, ContentPart::ToolResult { .. }) => tool_results += 1,
                _ => {}
            }
        }
        if tool_results > 0
            && (message.role != Role::User || tool_results != 1 || message.content.len() != 1)
        {
            return Err(ProviderError::invalid(
                "a ToolResult user message must contain exactly one ToolResult part",
            ));
        }
    }
    Ok(())
}

/// Resolves the secret only for the lifetime of one invocation.
///
/// # Errors
///
/// Returns a provider authentication error when a configured credential
/// reference cannot be resolved.
pub async fn invocation_from_revision(
    revision: &crate::AgentRevision,
    request_id: impl Into<String>,
    request: ProviderRequest,
    resolver: Arc<dyn CredentialResolver>,
    cancellation: CancellationToken,
) -> Result<ProviderInvocation, ProviderError> {
    let credential = match &revision.definition.credential_ref {
        Some(reference) => Some(resolver.resolve(reference).await?),
        None => None,
    };
    Ok(ProviderInvocation {
        request_id: request_id.into(),
        model: revision.definition.model.clone(),
        endpoint: revision.definition.endpoint.clone(),
        credential,
        request,
        cancellation,
    })
}
