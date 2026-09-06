//! Provider catalog, Agent configuration and Session admission.
use super::{
    AgentConfiguration, AgentMode, AgentProvider, AgentProviderGateway, AgentProviderView,
    AgentView, ApiError, Arc, Command, CommandResult, ControlStoreError, DomainError, ErrorCode,
    HashMap, HashSet, LocalControlService, Mutex, PendingEvent, ProviderMessage, ProviderModel,
    RunView, SessionView, State, Uuid, Value, Weak, WorkspaceAgentResponse, decode_state, error,
    json, pending, require_agent, serialization_error, store_error,
};
use ait_contracts::ProviderSecret;

pub(super) fn builtin_providers() -> Vec<AgentProviderView> {
    [
        AgentMode::Codex,
        AgentMode::Echo,
        AgentMode::Tool,
        AgentMode::Manual,
        AgentMode::ProviderFailure,
        AgentMode::ApprovalRequired,
    ]
    .into_iter()
    .map(|kind| {
        let key = serde_json::to_value(kind)
            .expect("provider kind")
            .as_str()
            .expect("string")
            .to_owned();
        let models = if kind == AgentMode::Codex {
            ["gpt-5.6-sol"]
                .into_iter()
                .map(|id| ProviderModel {
                    id: id.into(),
                    name: id.into(),
                    reasoning_efforts: ["low", "medium", "high", "xhigh", "max", "ultra"]
                        .map(str::to_owned)
                        .to_vec(),
                })
                .collect()
        } else {
            vec![ProviderModel {
                id: "default".into(),
                name: "Default".into(),
                reasoning_efforts: Vec::new(),
            }]
        };
        AgentProviderView {
            provider: AgentProvider {
                id: format!("builtin-{key}"),
                name: if kind == AgentMode::Codex {
                    "Codex".into()
                } else {
                    key
                },
                kind,
                url: None,
                models,
            },
            has_secret: false,
        }
    })
    .collect()
}

fn invalid(message: &str) -> ApiError {
    error(ErrorCode::InvalidAgentConfiguration, message, false)
}

pub(super) fn validate_provider(provider: &AgentProvider) -> Result<(), ApiError> {
    if provider.id.trim().is_empty() || provider.name.trim().is_empty() {
        return Err(invalid("provider id and name are required"));
    }
    if let Some(url) = &provider.url {
        let parsed = url::Url::parse(url).map_err(|_| invalid("invalid provider URL"))?;
        if !matches!(parsed.scheme(), "http" | "https")
            || parsed.host_str().is_none()
            || !parsed.username().is_empty()
            || parsed.password().is_some()
            || parsed.query().is_some()
            || parsed.fragment().is_some()
        {
            return Err(invalid(
                "provider URL must be HTTP(S), without credentials, query or fragment",
            ));
        }
    }
    let mut ids = HashSet::new();
    for model in &provider.models {
        let mut efforts = HashSet::new();
        if model.id.trim().is_empty()
            || model.name.trim().is_empty()
            || !ids.insert(&model.id)
            || model
                .reasoning_efforts
                .iter()
                .any(|effort| effort.trim().is_empty() || !efforts.insert(effort))
        {
            return Err(invalid(
                "model identifiers and reasoning efforts must be nonempty and unique",
            ));
        }
    }
    Ok(())
}

pub(super) fn validate_config<'a>(
    state: &'a State,
    config: &AgentConfiguration,
) -> Result<&'a AgentProvider, ApiError> {
    let provider = state
        .providers
        .iter()
        .find(|p| p.provider.id == config.provider_id)
        .map(|p| &p.provider)
        .ok_or_else(|| invalid("Agent provider not found"))?;
    validate_model(provider, config)?;
    Ok(provider)
}

fn validate_model(provider: &AgentProvider, config: &AgentConfiguration) -> Result<(), ApiError> {
    let model = provider
        .models
        .iter()
        .find(|m| m.id == config.model)
        .ok_or_else(|| invalid("model is not in the provider catalog"))?;
    if config
        .reasoning_effort
        .as_ref()
        .is_some_and(|effort| !model.reasoning_efforts.contains(effort))
    {
        return Err(invalid(
            "reasoning effort is not supported by the selected model",
        ));
    }
    Ok(())
}

pub(super) fn provider_kind(
    state: &State,
    config: &AgentConfiguration,
) -> Result<AgentMode, ApiError> {
    Ok(validate_config(state, config)?.kind)
}

pub(super) fn require_named_agent<'a>(
    state: &'a State,
    id: &str,
) -> Result<&'a AgentView, ApiError> {
    let agent = require_agent(state, id)?;
    if agent.owner_session_id.is_some() {
        return Err(invalid("a named Agent preset is required"));
    }
    Ok(agent)
}

pub(super) fn register_agent(
    state: &mut State,
    id: String,
    name: String,
    config: AgentConfiguration,
) -> Result<(CommandResult, Vec<PendingEvent>), ApiError> {
    if id.trim().is_empty() || name.trim().is_empty() || state.agents.iter().any(|a| a.id == id) {
        return Err(invalid("Agent id and name are required; id must be unique"));
    }
    validate_config(state, &config)?;
    let agent = AgentView {
        id: id.clone(),
        name,
        config,
        owner_session_id: None,
        revision: 1,
        enabled: true,
    };
    state.agents.push(agent.clone());
    Ok((
        CommandResult::Agent(agent.clone()),
        vec![pending("agent.registered", Some(id), &agent)],
    ))
}

pub(super) fn update_agent(
    state: &mut State,
    id: &str,
    name: String,
    config: AgentConfiguration,
) -> Result<(CommandResult, Vec<PendingEvent>), ApiError> {
    require_named_agent(state, id)?;
    if name.trim().is_empty() {
        return Err(invalid("preset name is required"));
    }
    validate_config(state, &config)?;
    let agent = state
        .agents
        .iter_mut()
        .find(|a| a.id == id)
        .expect("validated Agent");
    agent.name = name;
    agent.config = config;
    agent.revision += 1;
    Ok((
        CommandResult::Agent(agent.clone()),
        vec![pending("agent.updated", Some(id.into()), agent)],
    ))
}

pub(super) fn agent_for_session(
    state: &mut State,
    agent_id: &str,
    session_id: &str,
) -> Result<String, ApiError> {
    let agent = require_agent(state, agent_id)?;
    if agent
        .owner_session_id
        .as_deref()
        .is_none_or(|owner| owner == session_id)
    {
        return Ok(agent_id.into());
    }
    let mut copy = agent.clone();
    copy.id = Uuid::new_v4().to_string();
    copy.owner_session_id = Some(session_id.into());
    copy.revision = 1;
    let id = copy.id.clone();
    state.agents.push(copy);
    Ok(id)
}

pub(super) fn set_session_config(
    state: &mut State,
    session_id: &str,
    config: AgentConfiguration,
) -> Result<(CommandResult, Vec<PendingEvent>), ApiError> {
    validate_config(state, &config)?;
    let index = state
        .sessions
        .iter()
        .position(|s| s.id == session_id)
        .ok_or_else(|| error(ErrorCode::SessionNotFound, "session not found", false))?;
    ensure_idle(&state.sessions[index])?;
    let current = require_agent(state, &state.sessions[index].agent_id)?.clone();
    let agent = if current.owner_session_id.as_deref() == Some(session_id) {
        let target = state
            .agents
            .iter_mut()
            .find(|a| a.id == current.id)
            .expect("current Agent");
        target.config = config;
        target.revision += 1;
        target.clone()
    } else {
        let agent = AgentView {
            id: Uuid::new_v4().to_string(),
            name: String::new(),
            config,
            owner_session_id: Some(session_id.into()),
            revision: 1,
            enabled: true,
        };
        state.agents.push(agent.clone());
        agent
    };
    state.sessions[index].agent_id.clone_from(&agent.id);
    state.sessions[index].version += 1;
    let session = &state.sessions[index];
    Ok((
        CommandResult::Session(session.clone()),
        vec![
            pending("agent.updated", Some(agent.id.clone()), &agent),
            pending("session.agent_updated", Some(session_id.into()), session),
        ],
    ))
}

fn command_session(command: &Command) -> Option<&str> {
    match command {
        Command::SendMessage { session_id, .. }
        | Command::SetSessionAgent { session_id, .. }
        | Command::SetSessionConfig { session_id, .. } => Some(session_id),
        Command::ForkSession { id, .. } | Command::CreateSession { id, .. } => Some(id),
        _ => None,
    }
}

fn busy() -> ApiError {
    error(
        ErrorCode::SessionBusy,
        "session already has an active run or operation",
        false,
    )
}
fn ensure_idle(session: &SessionView) -> Result<(), ApiError> {
    if session.active_run_id.is_some() {
        Err(busy())
    } else {
        Ok(())
    }
}
pub(super) fn check_session_admission(state: &State, command: &Command) -> Result<(), ApiError> {
    if let Some(id) = command_session(command)
        && let Some(session) = state.sessions.iter().find(|s| s.id == id)
    {
        ensure_idle(session)?;
    }
    Ok(())
}

impl LocalControlService {
    pub(super) fn acquire_session(&self, command: &Command) -> Result<Option<Arc<()>>, ApiError> {
        let Some(id) = command_session(command) else {
            return Ok(None);
        };
        let mut leases = self.session_leases.lock().map_err(|_| busy())?;
        leases.retain(|_, lease| lease.strong_count() > 0);
        if leases.get(id).and_then(Weak::upgrade).is_some() {
            return Err(busy());
        }
        let lease = Arc::new(());
        leases.insert(id.into(), Arc::downgrade(&lease));
        Ok(Some(lease))
    }

    fn gateway(&self) -> Result<&dyn AgentProviderGateway, ApiError> {
        self.provider_gateway
            .as_deref()
            .ok_or_else(|| invalid("provider gateway is not configured"))
    }

    pub(super) async fn save_provider(
        &self,
        provider: AgentProvider,
        secret: Option<ProviderSecret>,
    ) -> Result<CommandResult, ApiError> {
        validate_provider(&provider)?;
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
            let snapshot = self.store.load().await.map_err(store_error)?;
            let mut state = decode_state(snapshot.value)?;
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
            match self
                .store
                .commit(
                    snapshot.revision,
                    serde_json::to_value(state).map_err(serialization_error)?,
                    events,
                )
                .await
            {
                Ok(_) => return Ok(CommandResult::AgentProvider(view)),
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

    pub(super) async fn discover_provider_models(
        &self,
        mut provider: AgentProvider,
        secret: Option<ProviderSecret>,
    ) -> Result<CommandResult, ApiError> {
        validate_provider(&provider)?;
        if !matches!(provider.kind, AgentMode::OpenAI | AgentMode::DeepSeek) {
            return Err(invalid("this provider does not expose model discovery"));
        }
        let state = decode_state(self.store.load().await.map_err(store_error)?.value)?;
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
        for model in &mut models {
            if let Some(old) =
                existing.and_then(|p| p.provider.models.iter().find(|m| m.id == model.id))
            {
                model.reasoning_efforts.clone_from(&old.reasoning_efforts);
            }
        }
        provider.models = models;
        validate_provider(&provider)?;
        Ok(CommandResult::ProviderModels(provider.models))
    }

    pub(super) async fn refresh_provider(
        &self,
        provider_id: &str,
    ) -> Result<CommandResult, ApiError> {
        let state = decode_state(self.store.load().await.map_err(store_error)?.value)?;
        let provider = state
            .providers
            .iter()
            .find(|p| p.provider.id == provider_id)
            .ok_or_else(|| invalid("provider not found"))?;
        let reference = state
            .provider_credentials
            .get(provider_id)
            .ok_or_else(|| invalid("provider secret is not configured"))?;
        let models = self
            .gateway()?
            .list_models(&provider.provider, reference)
            .await
            .map_err(domain_error)?;
        // Apply fetched IDs to the latest configuration, preserving manually declared
        // capabilities because standard model-list APIs do not advertise effort levels.
        for _ in 0..4 {
            let snapshot = self.store.load().await.map_err(store_error)?;
            let mut latest = decode_state(snapshot.value)?;
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
            for model in &mut refreshed {
                if let Some(old) = target.provider.models.iter().find(|m| m.id == model.id) {
                    model.reasoning_efforts.clone_from(&old.reasoning_efforts);
                }
            }
            target.provider.models = refreshed;
            validate_provider(&target.provider)?;
            let view = target.clone();
            let events = vec![pending(
                "agent_provider.updated",
                Some(provider_id.into()),
                &view,
            )];
            match self
                .store
                .commit(
                    snapshot.revision,
                    serde_json::to_value(latest).map_err(serialization_error)?,
                    events,
                )
                .await
            {
                Ok(_) => return Ok(CommandResult::AgentProvider(view)),
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

    pub(super) async fn invoke_provider(
        &self,
        state: &State,
        run: &RunView,
    ) -> Result<WorkspaceAgentResponse, DomainError> {
        let gateway = self.provider_gateway.as_deref().ok_or_else(|| {
            DomainError::invariant(
                ErrorCode::InvalidConfiguration,
                "provider gateway is not configured",
            )
        })?;
        let reference = state.run_credentials.get(&run.id).ok_or_else(|| {
            DomainError::invariant(
                ErrorCode::InvalidConfiguration,
                "provider secret is not configured",
            )
        })?;
        let mut messages = Vec::new();
        let mut current = Some(run.base_message_id.as_str());
        while let Some(id) = current {
            let message = state.messages.iter().find(|m| m.id == id).ok_or_else(|| {
                DomainError::invariant(ErrorCode::MessageNotFound, "message path is incomplete")
            })?;
            if let Some(text) = &message.text {
                messages.push(ProviderMessage {
                    role: message.role.clone(),
                    text: text.clone(),
                });
            }
            current = message.parent_message_id.as_deref();
        }
        messages.reverse();
        let assistant_text = gateway
            .complete(&run.provider, reference, &run.config, messages)
            .await?;
        Ok(WorkspaceAgentResponse {
            assistant_text,
            commit_id: None,
        })
    }
}

fn domain_error(failure: DomainError) -> ApiError {
    error(failure.code, failure.message, failure.retryable)
}

/// Upgrade legacy JSON snapshots in memory; the next atomic commit stores v3.
pub(super) fn migrate_state(mut value: Value) -> Result<Value, ApiError> {
    if value.get("providers").is_some() {
        return Ok(value);
    }
    let mut providers = builtin_providers();
    if let Some(agents) = value.get_mut("agents").and_then(Value::as_array_mut) {
        for agent in agents {
            let kind: AgentMode =
                serde_json::from_value(agent["mode"].clone()).map_err(serialization_error)?;
            let model = agent["model"].as_str().unwrap_or("default").to_owned();
            let provider = providers
                .iter_mut()
                .find(|p| p.provider.kind == kind)
                .ok_or_else(|| invalid("legacy provider kind is unavailable"))?;
            if !provider.provider.models.iter().any(|m| m.id == model) {
                provider.provider.models.push(ProviderModel {
                    id: model.clone(),
                    name: model.clone(),
                    reasoning_efforts: Vec::new(),
                });
            }
            agent["config"] = json!({"provider_id": provider.provider.id, "model": model, "reasoning_effort": null});
            agent.as_object_mut().expect("Agent object").remove("model");
            agent.as_object_mut().expect("Agent object").remove("mode");
        }
    }
    let configs: HashMap<String, Value> = value["agents"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|a| Some((a["id"].as_str()?.into(), a["config"].clone())))
        .collect();
    if let Some(runs) = value.get_mut("runs").and_then(Value::as_array_mut) {
        for run in runs {
            let mut config = configs
                .get(run["agent_id"].as_str().unwrap_or_default())
                .cloned()
                .ok_or_else(|| invalid("legacy Run Agent is unavailable"))?;
            config["reasoning_effort"] = run["reasoning_effort"].clone();
            let provider = providers
                .iter()
                .find(|p| Some(p.provider.id.as_str()) == config["provider_id"].as_str())
                .expect("migrated provider");
            run["provider"] =
                serde_json::to_value(&provider.provider).map_err(serialization_error)?;
            run["config"] = config;
            run.as_object_mut()
                .expect("Run object")
                .remove("reasoning_effort");
        }
    }
    value["providers"] = serde_json::to_value(providers).map_err(serialization_error)?;
    Ok(value)
}

pub(super) struct InvocationGuard<'a> {
    cancellations: &'a Mutex<HashMap<String, tokio_util::sync::CancellationToken>>,
    id: String,
}
impl<'a> InvocationGuard<'a> {
    pub(super) fn new(
        cancellations: &'a Mutex<HashMap<String, tokio_util::sync::CancellationToken>>,
        id: &str,
        token: tokio_util::sync::CancellationToken,
    ) -> Self {
        cancellations
            .lock()
            .expect("cancellations")
            .insert(id.into(), token);
        Self {
            cancellations,
            id: id.into(),
        }
    }
}
impl Drop for InvocationGuard<'_> {
    fn drop(&mut self) {
        self.cancellations
            .lock()
            .expect("cancellations")
            .remove(&self.id);
    }
}
