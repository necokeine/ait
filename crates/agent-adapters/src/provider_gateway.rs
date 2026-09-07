//! OS credential storage and Rig-backed provider operations.
use crate::{
    LLMClient, LLMClientConfig, LLMProvider,
    llm::{AssistantContent, CompletionRequest, Message},
};
use ait_domain::{
    AgentConfiguration, AgentProvider, DomainError, ErrorCode, ProviderKind, ProviderModel,
};
use ait_ports::{AgentProviderGateway, ProviderMessage};
use async_trait::async_trait;

/// Uses the operating system credential store; no API key enters workspace state.
#[derive(Default)]
pub struct RigProviderGateway;

fn credential_error() -> DomainError {
    DomainError::invariant(
        ErrorCode::InvalidConfiguration,
        "provider credential store is unavailable or credential is missing",
    )
}

fn provider_error() -> DomainError {
    DomainError::invariant(
        ErrorCode::ProviderFailed,
        "provider request failed; check connection, model and credentials",
    )
}

async fn client(provider: &AgentProvider, reference: &str) -> Result<LLMClient, DomainError> {
    let reference = reference.to_owned();
    let secret = tokio::task::spawn_blocking(move || {
        keyring::Entry::new("ait.agent-provider", &reference).and_then(|entry| entry.get_password())
    })
    .await
    .map_err(|_| credential_error())?
    .map_err(|_| credential_error())?;
    client_with_secret(provider, secret)
}

fn client_with_secret(provider: &AgentProvider, secret: String) -> Result<LLMClient, DomainError> {
    let kind = match provider.kind {
        ProviderKind::OpenAI => LLMProvider::OpenAI,
        ProviderKind::DeepSeek => LLMProvider::DeepSeek,
        ProviderKind::Codex => {
            return Err(DomainError::invariant(
                ErrorCode::InvalidConfiguration,
                "provider does not expose an LLM API",
            ));
        }
    };
    let mut config = LLMClientConfig::new(kind, secret);
    config.base_url.clone_from(&provider.url);
    LLMClient::new(config).map_err(|_| provider_error())
}

async fn discover_models(client: LLMClient) -> Result<Vec<ProviderModel>, DomainError> {
    let models = client.list_models().await.map_err(|_| provider_error())?;
    Ok(models
        .data
        .into_iter()
        .map(|model| ProviderModel {
            name: model.name.unwrap_or_else(|| model.id.clone()),
            id: model.id,
            reasoning_efforts: Vec::new(),
        })
        .collect())
}

fn text_request(
    client: &LLMClient,
    config: &AgentConfiguration,
    messages: Vec<ProviderMessage>,
) -> Result<CompletionRequest, DomainError> {
    let mut history: Vec<_> = messages
        .into_iter()
        .map(|message| match message.role.as_str() {
            "system" => Message::system(message.text),
            "assistant" => Message::assistant(message.text),
            _ => Message::user(message.text),
        })
        .collect();
    let current = history.pop().ok_or_else(provider_error)?;
    let mut request = client.completion_request_with_history(&config.model, history, current);
    // This port returns text only. A persisted host tool loop must precede any
    // exposure of function definitions through the gateway.
    request.tools.clear();
    Ok(request)
}

#[async_trait]
impl AgentProviderGateway for RigProviderGateway {
    async fn store_secret(&self, reference: &str, secret: &str) -> Result<(), DomainError> {
        let reference = reference.to_owned();
        let secret = secret.to_owned();
        tokio::task::spawn_blocking(move || {
            keyring::Entry::new("ait.agent-provider", &reference)
                .and_then(|entry| entry.set_password(&secret))
        })
        .await
        .map_err(|_| credential_error())?
        .map_err(|_| credential_error())
    }

    async fn delete_secret(&self, reference: &str) -> Result<(), DomainError> {
        let reference = reference.to_owned();
        tokio::task::spawn_blocking(move || {
            keyring::Entry::new("ait.agent-provider", &reference)
                .and_then(|entry| entry.delete_credential())
        })
        .await
        .map_err(|_| credential_error())?
        .map_err(|_| credential_error())
    }

    async fn list_models(
        &self,
        provider: &AgentProvider,
        credential_ref: &str,
    ) -> Result<Vec<ProviderModel>, DomainError> {
        discover_models(client(provider, credential_ref).await?).await
    }

    async fn list_models_with_secret(
        &self,
        provider: &AgentProvider,
        secret: &str,
    ) -> Result<Vec<ProviderModel>, DomainError> {
        discover_models(client_with_secret(provider, secret.to_owned())?).await
    }

    async fn complete(
        &self,
        provider: &AgentProvider,
        credential_ref: &str,
        config: &AgentConfiguration,
        messages: Vec<ProviderMessage>,
    ) -> Result<String, DomainError> {
        let client = client(provider, credential_ref).await?;
        let mut request = text_request(&client, config, messages)?;
        if let Some(effort) = &config.reasoning_effort {
            request.additional_params = Some(match provider.kind {
                ProviderKind::OpenAI => serde_json::json!({"reasoning": {"effort": effort}}),
                _ => serde_json::json!({"reasoning_effort": effort}),
            });
        }
        let response = client
            .complete(request)
            .await
            .map_err(|_| provider_error())?;
        let text = response
            .choice
            .into_iter()
            .filter_map(|part| match part {
                AssistantContent::Text(text) => Some(text.text),
                _ => None,
            })
            .collect::<String>();
        if text.is_empty() {
            return Err(provider_error());
        }
        Ok(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ait_tools::DEFAULT_SYSTEM_PROMPT;

    #[test]
    fn gateway_preserves_history_after_default_system_without_advertising_tools() {
        let config = AgentConfiguration {
            model: "fixture-model".into(),
            ..Default::default()
        };
        for provider in [LLMProvider::DeepSeek, LLMProvider::OpenAI] {
            let client = LLMClient::new(LLMClientConfig::new(provider, "fixture-key")).unwrap();
            let messages = [
                ("system", "Project instructions"),
                ("user", "First"),
                ("assistant", "Answer"),
                ("user", "Current {{literal}}"),
            ]
            .into_iter()
            .map(|(role, text)| ProviderMessage {
                role: role.into(),
                text: text.into(),
            })
            .collect();
            let request = text_request(&client, &config, messages).unwrap();
            assert!(request.tools.is_empty());
            assert_eq!(
                request.chat_history,
                vec![
                    Message::system(DEFAULT_SYSTEM_PROMPT),
                    Message::system("Project instructions"),
                    Message::user("First"),
                    Message::assistant("Answer"),
                    Message::user("Current {{literal}}"),
                ]
            );
            assert!(text_request(&client, &config, vec![]).is_err());
        }
    }
}
