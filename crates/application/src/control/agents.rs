//! Provider catalog, Agent configuration and Session admission.
use super::{
    AgentConfiguration, AgentMode, AgentProvider, AgentProviderGateway, AgentProviderView,
    AgentView, ApiError, Arc, Command, CommandResult, ControlStoreError, DomainError, ErrorCode,
    HashMap, HashSet, HostProviderModelCatalog, LocalControlService, Mutex, PendingEvent,
    ProviderMessage, ProviderModel, RunView, SessionView, Uuid, Value, Weak, WorkingSet,
    WorkspaceAgentResponse, error, json, pending, require_agent, serialization_error, store_error,
};
use ait_contracts::ProviderSecret;

pub(super) fn builtin_providers() -> Vec<AgentProviderView> {
    vec![AgentProviderView {
        provider: AgentProvider {
            id: "builtin-codex".into(),
            name: "Codex".into(),
            kind: AgentMode::Codex,
            url: None,
            models: vec![ProviderModel {
                id: "gpt-5.6-sol".into(),
                name: "gpt-5.6-sol".into(),
                reasoning_efforts: ["low", "medium", "high", "xhigh", "max", "ultra"]
                    .map(str::to_owned)
                    .to_vec(),
            }],
        },
        has_secret: false,
    }]
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
    state: &'a WorkingSet,
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

pub(super) fn require_named_agent<'a>(
    state: &'a WorkingSet,
    id: &str,
) -> Result<&'a AgentView, ApiError> {
    let agent = require_agent(state, id)?;
    if agent.owner_session_id.is_some() {
        return Err(invalid("a named Agent preset is required"));
    }
    Ok(agent)
}

pub(super) fn register_agent(
    state: &mut WorkingSet,
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
    state: &mut WorkingSet,
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
    state: &mut WorkingSet,
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
    state: &mut WorkingSet,
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
        Command::ForkSession { id, .. }
        | Command::DeriveSession { id, .. }
        | Command::CreateSession { id, .. } => Some(id),
        _ => None,
    }
}

pub(super) struct SessionAdmission {
    leases: Vec<(String, Arc<()>)>,
    derive_source_locked: bool,
}

impl SessionAdmission {
    pub(super) const fn derive_source_locked(&self) -> bool {
        self.derive_source_locked
    }

    pub(super) fn retain_for_session(&mut self, session_id: &str) {
        self.leases.retain(|(id, _)| id == session_id);
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
pub(super) fn check_session_admission(
    state: &WorkingSet,
    command: &Command,
) -> Result<(), ApiError> {
    if let Some(id) = command_session(command)
        && let Some(session) = state.sessions.iter().find(|s| s.id == id)
    {
        ensure_idle(session)?;
    }
    Ok(())
}

impl LocalControlService {
    pub(super) fn acquire_session(&self, command: &Command) -> Result<SessionAdmission, ApiError> {
        let Some(id) = command_session(command) else {
            return Ok(SessionAdmission {
                leases: Vec::new(),
                derive_source_locked: false,
            });
        };
        let mut leases = self.session_leases.lock().map_err(|_| busy())?;
        leases.retain(|_, lease| lease.strong_count() > 0);
        if leases.get(id).and_then(Weak::upgrade).is_some() {
            return Err(busy());
        }
        let lease = Arc::new(());
        leases.insert(id.into(), Arc::downgrade(&lease));
        let mut owned = vec![(id.to_owned(), lease)];
        let derive_source_locked = if let Command::DeriveSession {
            source_session_id, ..
        } = command
        {
            if source_session_id == id
                || leases
                    .get(source_session_id)
                    .and_then(Weak::upgrade)
                    .is_some()
            {
                false
            } else {
                let source_lease = Arc::new(());
                leases.insert(source_session_id.clone(), Arc::downgrade(&source_lease));
                owned.push((source_session_id.clone(), source_lease));
                true
            }
        } else {
            false
        };
        Ok(SessionAdmission {
            leases: owned,
            derive_source_locked,
        })
    }

    fn gateway(&self) -> Result<&dyn AgentProviderGateway, ApiError> {
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

    pub(super) async fn discover_provider_models(
        &self,
        mut provider: AgentProvider,
        secret: Option<ProviderSecret>,
    ) -> Result<CommandResult, ApiError> {
        validate_provider(&provider)?;
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
        let state = self
            .read_provider_records(provider_id, false)
            .await?
            .original;
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

    pub(super) async fn invoke_provider(
        &self,
        state: &WorkingSet,
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
            operations: Vec::new(),
            output_items: Vec::new(),
        })
    }
}

fn domain_error(failure: DomainError) -> ApiError {
    error(failure.code, failure.message, failure.retryable)
}

/// Upgrade legacy JSON snapshots in memory; the next atomic commit stores v3.
pub(super) fn migrate_state(mut value: Value) -> Result<Value, ApiError> {
    if value.get("providers").is_some() {
        remove_unused_retired_builtins(&mut value);
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

/// Older snapshots persist the whole builtin catalog, including unused adapters.
/// Retire only unreferenced entries; referenced or custom providers must still
/// decode normally so unsupported history is never silently dropped or rebound.
fn remove_unused_retired_builtins(value: &mut Value) {
    let mut referenced = HashSet::<String>::new();
    for collection in ["agents", "runs"] {
        for item in value[collection].as_array().into_iter().flatten() {
            for id in [
                item["config"]["provider_id"].as_str(),
                item["provider"]["id"].as_str(),
            ]
            .into_iter()
            .flatten()
            {
                referenced.insert(id.into());
            }
        }
    }
    if let Some(credentials) = value["provider_credentials"].as_object() {
        referenced.extend(credentials.keys().cloned());
    }
    if let Some(providers) = value["providers"].as_array_mut() {
        providers.retain(|view| {
            let (Some(id), Some(kind)) = (view["id"].as_str(), view["kind"].as_str()) else {
                return true;
            };
            id != format!("builtin-{kind}")
                || referenced.contains(id)
                || view["url"] != Value::Null
                || view["has_secret"] != false
                || serde_json::from_value::<AgentMode>(view["kind"].clone()).is_ok()
        });
    }
}

pub(super) struct InvocationGuard {
    cancellations: Arc<Mutex<HashMap<String, tokio_util::sync::CancellationToken>>>,
    id: String,
}
impl InvocationGuard {
    pub(super) fn new(
        cancellations: Arc<Mutex<HashMap<String, tokio_util::sync::CancellationToken>>>,
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
impl Drop for InvocationGuard {
    fn drop(&mut self) {
        self.cancellations
            .lock()
            .expect("cancellations")
            .remove(&self.id);
    }
}
