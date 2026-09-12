//! Provider catalog validation and Agent configuration reducers.
use crate::control::LocalControlService;
use crate::control::admission::ensure_idle;
use crate::control::errors::error;
use crate::control::events::pending;
use crate::control::state::WorkingSet;
use ait_contracts::{
    AgentConfiguration, AgentMode, AgentProvider, AgentProviderView, AgentView, ApiError,
    CommandResult, ProviderModel,
};
use ait_domain::ErrorCode;
use ait_ports::PendingEvent;
use std::collections::HashSet;
use uuid::Uuid;

pub(in crate::control) mod migration;
pub(in crate::control) mod providers;

pub(in crate::control) fn require_agent<'a>(
    state: &'a WorkingSet,
    id: &str,
) -> Result<&'a AgentView, ApiError> {
    state
        .agents
        .iter()
        .find(|agent| agent.id == id && agent.enabled)
        .ok_or_else(|| error(ErrorCode::AgentNotFound, "enabled agent not found", false))
}

pub(in crate::control) fn builtin_providers() -> Vec<AgentProviderView> {
    vec![
        AgentProviderView {
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
        },
        #[cfg(all(feature = "dev-mock-provider", debug_assertions))]
        AgentProviderView {
            provider: AgentProvider {
                id: "builtin-mock".into(),
                name: "Mock (Development)".into(),
                kind: AgentMode::Mock,
                url: None,
                models: vec![ProviderModel {
                    id: "mock-local".into(),
                    name: "Mock Local".into(),
                    reasoning_efforts: Vec::new(),
                }],
            },
            has_secret: false,
        },
    ]
}

pub(in crate::control) fn invalid(message: &str) -> ApiError {
    error(ErrorCode::InvalidAgentConfiguration, message, false)
}

pub(in crate::control) fn validate_provider(provider: &AgentProvider) -> Result<(), ApiError> {
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

pub(in crate::control) fn validate_config<'a>(
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

pub(in crate::control) fn validate_model(
    provider: &AgentProvider,
    config: &AgentConfiguration,
) -> Result<(), ApiError> {
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

pub(in crate::control) fn preserve_unadvertised_reasoning_efforts(
    models: &mut [ProviderModel],
    existing: Option<&AgentProvider>,
) {
    for model in models {
        if model.reasoning_efforts.is_empty()
            && let Some(old) = existing.and_then(|provider| {
                provider
                    .models
                    .iter()
                    .find(|candidate| candidate.id == model.id)
            })
        {
            model.reasoning_efforts.clone_from(&old.reasoning_efforts);
        }
    }
}

pub(in crate::control) fn require_named_agent<'a>(
    state: &'a WorkingSet,
    id: &str,
) -> Result<&'a AgentView, ApiError> {
    let agent = require_agent(state, id)?;
    if agent.owner_session_id.is_some() {
        return Err(invalid("a named Agent preset is required"));
    }
    Ok(agent)
}

pub(in crate::control) fn register_agent(
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

pub(in crate::control) fn update_agent(
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

pub(in crate::control) fn agent_for_session(
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

pub(in crate::control) fn set_session_config(
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

impl LocalControlService {
    #[cfg(all(feature = "dev-mock-provider", debug_assertions))]
    pub(in crate::control) fn invoke_mock() -> ait_ports::WorkspaceAgentResponse {
        ait_ports::WorkspaceAgentResponse {
            assistant_text: "Mock assistant response.".into(),
            commit_id: None,
            operations: Vec::new(),
            output_items: Vec::new(),
        }
    }
}
