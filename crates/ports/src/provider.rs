//! Credential storage and remote LLM operations behind the application boundary.

use ait_domain::{AgentConfiguration, AgentProvider, DomainError, ProviderModel};
use async_trait::async_trait;

/// A provider adapter resolves credential references without exposing secrets to state.
#[async_trait]
pub trait AgentProviderGateway: Send + Sync {
    /// Store a credential under an opaque, immutable reference.
    async fn store_secret(&self, reference: &str, secret: &str) -> Result<(), DomainError>;
    /// Delete an unused credential after a failed configuration transaction.
    async fn delete_secret(&self, reference: &str) -> Result<(), DomainError>;
    /// Fetch model identifiers; advertised effort metadata can be absent.
    async fn list_models(
        &self,
        provider: &AgentProvider,
        credential_ref: &str,
    ) -> Result<Vec<ProviderModel>, DomainError>;
    /// Preview models using an unsaved credential, without writing it to storage.
    async fn list_models_with_secret(
        &self,
        provider: &AgentProvider,
        secret: &str,
    ) -> Result<Vec<ProviderModel>, DomainError>;
    /// Generate one structured turn. The host owns persistence and tool execution.
    async fn complete_turn(
        &self,
        provider: &AgentProvider,
        credential_ref: &str,
        config: &AgentConfiguration,
        request: crate::AgentInvocation,
        executable_tools: Vec<String>,
    ) -> Result<crate::AgentResponse, DomainError> {
        if !executable_tools.is_empty() {
            return Err(DomainError::invariant(
                ait_domain::ErrorCode::InvalidConfiguration,
                "provider gateway does not support host tools",
            ));
        }
        let messages = request
            .message_path
            .into_iter()
            .filter_map(|entry| {
                let ait_domain::ProjectedMessage::Visible(message) = entry else {
                    return None;
                };
                Some(ProviderMessage {
                    role: serde_json::to_value(message.role).ok()?.as_str()?.into(),
                    text: message
                        .sub_messages
                        .into_iter()
                        .filter_map(|part| match part {
                            ait_domain::SubMessage::Text { text } => Some(text),
                            _ => None,
                        })
                        .collect(),
                })
            })
            .collect();
        Ok(crate::AgentResponse {
            sub_messages: vec![ait_domain::SubMessage::Text {
                text: self
                    .complete(provider, credential_ref, config, messages)
                    .await?,
            }],
            usage: ait_domain::RunUsage::default(),
        })
    }
    /// Invoke one LLM turn against the fixed Agent configuration.
    async fn complete(
        &self,
        provider: &AgentProvider,
        credential_ref: &str,
        config: &AgentConfiguration,
        messages: Vec<ProviderMessage>,
    ) -> Result<String, DomainError>;
}

/// Model discovery for providers authenticated by a host application rather
/// than an API credential managed by Ait.
#[async_trait]
pub trait HostProviderModelCatalog: Send + Sync {
    /// Fetch the provider's picker-visible models and their advertised
    /// capabilities without changing persisted configuration.
    async fn discover_models(
        &self,
        provider: &AgentProvider,
    ) -> Result<Vec<ProviderModel>, DomainError>;
}

/// Text history projected for a single LLM call, without an SDK dependency.
#[derive(Clone, Debug)]
pub struct ProviderMessage {
    /// Domain role: system, user, or assistant.
    pub role: String,
    /// Text content.
    pub text: String,
}
