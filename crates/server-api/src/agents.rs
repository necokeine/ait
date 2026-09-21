use serde_json::Value;
use server_application::agents::{AgentError, Agents};
use server_domain::agent::{AgentConfig, AgentSnapshot, AgentTarget, Revision};
use server_protocol::{ErrorCode, agent};

use crate::Shared;

pub(super) async fn dispatch(
    method: &str,
    params: Value,
    state: &Shared,
) -> Result<Value, ErrorCode> {
    let method = method.to_owned();
    crate::jobs::run(
        state,
        state.agents.clone(),
        ErrorCode::AgentIo,
        move |agents| execute(agents, &method, params),
    )
    .await
}

fn execute(agents: &mut Agents, method: &str, params: Value) -> Result<Value, ErrorCode> {
    match method {
        "agent.configure" => {
            let request: agent::Configure = decode(params)?;
            let target = match (request.agent_id, request.expected_revision) {
                (None, None) => AgentTarget::Create,
                (Some(id), Some(expected)) => AgentTarget::Update {
                    id: id.parse().map_err(|_| ErrorCode::InvalidMessage)?,
                    expected: Revision::new(expected).map_err(|_| ErrorCode::InvalidMessage)?,
                },
                _ => return Err(ErrorCode::InvalidMessage),
            };
            let config = request.config;
            let config = AgentConfig::new(
                config.name,
                config
                    .driver_type
                    .parse()
                    .map_err(|_| ErrorCode::InvalidMessage)?,
                config.model,
                config
                    .credential_ref
                    .as_deref()
                    .map(str::parse)
                    .transpose()
                    .map_err(|_| ErrorCode::InvalidMessage)?,
                config.enabled,
            )
            .map_err(|_| ErrorCode::InvalidMessage)?;
            let receipt = agents
                .configure(target, config, &request.idempotency_key)
                .map_err(error)?;
            encode(agent::Receipt {
                operation_id: receipt.operation_id.to_string(),
                agent_id: receipt.agent_id.to_string(),
                revision: receipt.revision.value(),
            })
        }
        "agent.get" => {
            let request: agent::Get = decode(params)?;
            let id = request
                .agent_id
                .parse()
                .map_err(|_| ErrorCode::InvalidMessage)?;
            let revision = request
                .revision
                .map(Revision::new)
                .transpose()
                .map_err(|_| ErrorCode::InvalidMessage)?;
            encode(view(&agents.get(id, revision).map_err(error)?))
        }
        "agent.list" => {
            let request: agent::List = decode(params)?;
            let after = request
                .after
                .as_deref()
                .map(str::parse)
                .transpose()
                .map_err(|_| ErrorCode::InvalidMessage)?;
            let agents = agents.list(after, request.limit).map_err(error)?;
            let next_after = (agents.len() == request.limit)
                .then(|| agents.last().map(|agent| agent.id().to_string()))
                .flatten();
            encode(agent::Page {
                agents: agents.iter().map(view).collect(),
                next_after,
            })
        }
        "agent.default.get" => {
            let _: agent::GetDefault = decode(params)?;
            let selection = agents.get_default().map_err(error)?;
            encode(agent::DefaultSelection {
                agent_id: selection.agent_id.map(|id| id.to_string()),
                version: selection.version,
            })
        }
        "agent.default.set" => {
            let request: agent::SetDefault = decode(params)?;
            let id = request
                .agent_id
                .as_deref()
                .map(str::parse)
                .transpose()
                .map_err(|_| ErrorCode::InvalidMessage)?;
            let receipt = agents
                .set_default(id, request.expected_version, &request.idempotency_key)
                .map_err(error)?;
            encode(agent::DefaultReceipt {
                operation_id: receipt.operation_id.to_string(),
                selection: agent::DefaultSelection {
                    agent_id: receipt.selection.agent_id.map(|id| id.to_string()),
                    version: receipt.selection.version,
                },
            })
        }
        _ => Err(ErrorCode::MethodNotFound),
    }
}

fn decode<T: serde::de::DeserializeOwned>(params: Value) -> Result<T, ErrorCode> {
    serde_json::from_value(params).map_err(|_| ErrorCode::InvalidMessage)
}
fn encode(value: impl serde::Serialize) -> Result<Value, ErrorCode> {
    serde_json::to_value(value).map_err(|_| ErrorCode::AgentIo)
}

fn view(snapshot: &AgentSnapshot) -> agent::Agent {
    let config = snapshot.config();
    agent::Agent {
        agent_id: snapshot.id().to_string(),
        revision: snapshot.revision().value(),
        recorded_at: snapshot.recorded_at(),
        config: agent::Config {
            name: config.name().to_owned(),
            driver_type: config.driver().to_string(),
            model: config.model().to_owned(),
            credential_ref: config.credential_ref().map(ToString::to_string),
            enabled: config.enabled(),
        },
    }
}

fn error(error: AgentError) -> ErrorCode {
    match error {
        AgentError::Invalid => ErrorCode::InvalidMessage,
        AgentError::NotFound => ErrorCode::AgentNotFound,
        AgentError::RevisionNotFound => ErrorCode::AgentRevisionNotFound,
        AgentError::RevisionConflict => ErrorCode::AgentRevisionConflict,
        AgentError::DefaultConflict => ErrorCode::AgentDefaultConflict,
        AgentError::Disabled => ErrorCode::AgentDisabled,
        AgentError::IsDefault => ErrorCode::AgentIsDefault,
        AgentError::IdempotencyConflict => ErrorCode::IdempotencyConflict,
        AgentError::Busy => ErrorCode::CatalogBusy,
        AgentError::UnsupportedFormat => ErrorCode::UnsupportedFormat,
        AgentError::Io => ErrorCode::AgentIo,
    }
}

#[cfg(test)]
mod tests;
