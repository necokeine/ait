//! Provider credentials and host model discovery use cases.
use crate::control::LocalControlService;
use crate::control::catalog::{
    invalid, preserve_unadvertised_reasoning_efforts, validate_model, validate_provider,
};
use crate::control::errors::{error, store_error};
use crate::control::events::pending;
use ait_contracts::{
    AgentMode, AgentProvider, AgentProviderView, ApiError, CommandResult, ProviderSecret,
};
use ait_domain::{DomainError, ErrorCode};
use ait_ports::{AgentProviderGateway, ControlStoreError, HostProviderModelCatalog};
use uuid::Uuid;

fn domain_error(failure: DomainError) -> ApiError {
    error(failure.code, failure.message, failure.retryable)
}

impl LocalControlService {
    pub(in crate::control) fn gateway(&self) -> Result<&dyn AgentProviderGateway, ApiError> {
        self.provider_gateway
            .as_deref()
            .ok_or_else(|| invalid("provider gateway is not configured"))
    }

    fn host_catalog(&self) -> Result<&dyn HostProviderModelCatalog, ApiError> {
        self.host_provider_catalog.as_deref().ok_or_else(|| {
            error(
                ErrorCode::InvalidConfiguration,
                "host provider model catalog is not configured",
                false,
            )
        })
    }

    pub(in crate::control) async fn save_provider(
        &self,
        provider: AgentProvider,
        secret: Option<ProviderSecret>,
    ) -> Result<CommandResult, ApiError> {
        validate_provider(&provider)?;
        #[cfg(all(feature = "dev-mock-provider", debug_assertions))]
        if provider.kind == AgentMode::Mock {
            return Err(invalid("the development Mock provider is built in"));
        }
        let credential = if let Some(secret) = secret {
            if secret.0.trim().is_empty() {
                return Err(invalid("provider secret cannot be empty"));
            }
            if !matches!(provider.kind, AgentMode::OpenAI | AgentMode::DeepSeek) {
                return Err(invalid("this provider uses host authentication"));
            }
            let reference = Uuid::new_v4().to_string();
            self.gateway()?
                .store_secret(&reference, &secret.0)
                .await
                .map_err(domain_error)?;
            Some(reference)
        } else {
            None
        };
        let result = self.commit_provider(&provider, credential.as_deref()).await;
        if result.is_err()
            && let Some(reference) = credential
        {
            let _ = self.gateway()?.delete_secret(&reference).await;
        }
        result
    }

    async fn commit_provider(
        &self,
        provider: &AgentProvider,
        credential: Option<&str>,
    ) -> Result<CommandResult, ApiError> {
        for _ in 0..4 {
            let loaded = self.read_provider_records(&provider.id, true).await?;
            let mut state = loaded.original.clone();
            if let Some(existing) = state
                .providers
                .iter()
                .find(|p| p.provider.id == provider.id)
                && existing.provider.kind != provider.kind
            {
                return Err(invalid(
                    "provider kind cannot change; create another provider",
                ));
            }
            for agent in state
                .agents
                .iter()
                .filter(|a| a.config.provider_id == provider.id)
            {
                validate_model(provider, &agent.config)?;
            }
            if let Some(reference) = credential {
                state
                    .provider_credentials
                    .insert(provider.id.clone(), reference.into());
            }
            let view = AgentProviderView {
                provider: provider.clone(),
                has_secret: state.provider_credentials.contains_key(&provider.id),
            };
            if let Some(existing) = state
                .providers
                .iter_mut()
                .find(|p| p.provider.id == provider.id)
            {
                *existing = view.clone();
            } else {
                state.providers.push(view.clone());
            }
            let events = vec![pending(
                "agent_provider.updated",
                Some(provider.id.clone()),
                &view,
            )];
            match self.persist_records(&loaded, &state, events).await {
                Ok(()) => return Ok(CommandResult::AgentProvider(view)),
                Err(ControlStoreError::Conflict) => {}
                Err(failure) => return Err(store_error(failure)),
            }
        }
        Err(error(
            ErrorCode::RunQueueConflict,
            "concurrent provider update did not settle",
            true,
        ))
    }

    pub(in crate::control) async fn discover_provider_models(
        &self,
        mut provider: AgentProvider,
        secret: Option<ProviderSecret>,
    ) -> Result<CommandResult, ApiError> {
        validate_provider(&provider)?;
        #[cfg(all(feature = "dev-mock-provider", debug_assertions))]
        if provider.kind == AgentMode::Mock {
            return Err(invalid(
                "the development Mock provider has a fixed local model",
            ));
        }
        if provider.kind == AgentMode::Codex {
            if secret.is_some() {
                return Err(invalid("Codex uses host authentication"));
            }
            provider.models = self
                .host_catalog()?
                .discover_models(&provider)
                .await
                .map_err(domain_error)?;
            validate_provider(&provider)?;
            return Ok(CommandResult::ProviderModels(provider.models));
        }
        if !matches!(provider.kind, AgentMode::OpenAI | AgentMode::DeepSeek) {
            return Err(invalid("this provider does not expose model discovery"));
        }
        let state = self
            .read_provider_records(&provider.id, false)
            .await?
            .original;
        let existing = state
            .providers
            .iter()
            .find(|p| p.provider.id == provider.id);
        if existing.is_some_and(|p| p.provider.kind != provider.kind) {
            return Err(invalid(
                "provider kind cannot change; create another provider",
            ));
        }
        let mut models = if let Some(secret) = secret {
            if secret.0.trim().is_empty() {
                return Err(invalid("provider secret cannot be empty"));
            }
            self.gateway()?
                .list_models_with_secret(&provider, &secret.0)
                .await
        } else {
            let reference = state
                .provider_credentials
                .get(&provider.id)
                .ok_or_else(|| invalid("provider secret is not configured"))?;
            self.gateway()?.list_models(&provider, reference).await
        }
        .map_err(domain_error)?;
        preserve_unadvertised_reasoning_efforts(
            &mut models,
            existing.map(|provider| &provider.provider),
        );
        provider.models = models;
        validate_provider(&provider)?;
        Ok(CommandResult::ProviderModels(provider.models))
    }

    pub(in crate::control) async fn refresh_provider(
        &self,
        provider_id: &str,
    ) -> Result<CommandResult, ApiError> {
        let state = self
            .read_provider_records(provider_id, false)
            .await?
            .original;
        let provider = state
            .providers
            .iter()
            .find(|p| p.provider.id == provider_id)
            .ok_or_else(|| invalid("provider not found"))?;
        #[cfg(all(feature = "dev-mock-provider", debug_assertions))]
        if provider.provider.kind == AgentMode::Mock {
            return Err(invalid(
                "the development Mock provider has a fixed local model",
            ));
        }
        let reference = state
            .provider_credentials
            .get(provider_id)
            .ok_or_else(|| invalid("provider secret is not configured"))?;
        let models = self
            .gateway()?
            .list_models(&provider.provider, reference)
            .await
            .map_err(domain_error)?;
        // Apply fetched IDs to the latest configuration. Adapter-advertised
        // capabilities win; otherwise preserve manual declarations because standard
        // model-list APIs do not include effort levels.
        for _ in 0..4 {
            let loaded = self.read_provider_records(provider_id, false).await?;
            let mut latest = loaded.original.clone();
            let target = latest
                .providers
                .iter_mut()
                .find(|p| p.provider.id == provider_id)
                .ok_or_else(|| invalid("provider not found"))?;
            if target.provider.url != provider.provider.url
                || latest.provider_credentials.get(provider_id) != Some(reference)
            {
                return Err(invalid(
                    "provider connection changed during model discovery; refresh again",
                ));
            }
            let mut refreshed = models.clone();
            preserve_unadvertised_reasoning_efforts(&mut refreshed, Some(&target.provider));
            target.provider.models = refreshed;
            validate_provider(&target.provider)?;
            let view = target.clone();
            let events = vec![pending(
                "agent_provider.updated",
                Some(provider_id.into()),
                &view,
            )];
            match self.persist_records(&loaded, &latest, events).await {
                Ok(()) => return Ok(CommandResult::AgentProvider(view)),
                Err(ControlStoreError::Conflict) => {}
                Err(failure) => return Err(store_error(failure)),
            }
        }
        Err(error(
            ErrorCode::RunQueueConflict,
            "concurrent model refresh did not settle",
            true,
        ))
    }
}
