//! Concrete service state and crate-owned request dispatch.

use std::sync::{Arc, Mutex};

use server_model::outbound::QueueError;
use server_model::{Context, ErrorCode, Runtime};

use crate::capabilities::Group;

/// Services installed for this capability crate, sharing server-wide runtime resources.
#[derive(Debug)]
pub struct State {
    /// Shared Tokio admission, cancellation and task tracking.
    pub runtime: Arc<Runtime>,
    /// Installed agents service.
    pub agents: Option<Arc<Mutex<crate::service::agents::Agents>>>,
    /// Installed agent runtime service.
    pub agent_runtime: Option<Arc<Mutex<crate::service::agent_runtime::AgentRuntimeDirectory>>>,
    /// Installed native Agent executor.
    pub agent_execution: Option<crate::service::agent_execution::AgentExecution>,
    /// Whether the host can finish a coordinated Terminal close.
    pub has_terminals: bool,
}

impl std::ops::Deref for State {
    type Target = Runtime;

    fn deref(&self) -> &Runtime {
        &self.runtime
    }
}

pub(crate) mod agent_execution;
mod agent_runtime;

/// Remaining composition work after provider dispatch has completed.
pub enum Completion {
    /// Provider has delivered the response or registered a tracked completion wait.
    Complete,
    /// Close requested Terminals after Agent closure, then send the combined response.
    CloseTerminals {
        /// Correlation ID of the original request.
        request_id: String,
        /// Provider's completed portion of the response.
        value: serde_json::Value,
        /// Terminals to close using the terminal crate.
        terminal_ids: Vec<String>,
    },
}

/// Dispatch an admitted provider request using concrete shared request resources.
/// # Errors
/// Returns delivery failures; business failures are sent using the original request ID.
pub async fn dispatch(
    group: Group,
    mut context: Context<'_>,
    state: &State,
    connection: &mut crate::connection::Connection,
) -> Result<Completion, QueueError> {
    match group {
        Group::Agents => {
            context
                .rpc(
                    state.agents.clone(),
                    ErrorCode::AgentIo,
                    crate::rpc::agents::execute,
                )
                .await
        }
        Group::AgentRuntime => {
            let params = std::mem::take(&mut context.request.params);
            match agent_runtime::dispatch(&context.request.method, params, state).await {
                Ok(reply) if !reply.terminals.is_empty() => {
                    return Ok(Completion::CloseTerminals {
                        request_id: context.request.id,
                        value: reply.value,
                        terminal_ids: reply.terminals,
                    });
                }
                Ok(reply) => context.respond(Ok(reply.value)),
                Err(error) => context.respond(Err(error)),
            }
        }
        Group::AgentExecution if context.request.method == "agent.create.request" => {
            connection.create(context, state).await
        }
        Group::AgentExecution if context.request.method == "agent.finish.wait.request" => {
            agent_execution::wait(
                context.request.id,
                context.request.params,
                state,
                context.outbound,
            )
        }
        Group::Timeline if context.request.method == "agent.timeline.set_subscription.request" => {
            connection.subscribe(context, state).await
        }
        Group::Timeline if context.request.method == "agent.timeline.append.request" => {
            let Some(plugin) = connection.plugin() else {
                context.respond(Err(ErrorCode::UnsupportedCapability))?;
                return Ok(Completion::Complete);
            };
            let payload = serde_json::json!({"request":context.request.params,"plugin":plugin});
            let result =
                agent_execution::dispatch("internal.timeline.append", payload, state).await;
            context.respond(result)
        }
        Group::AgentExecution | Group::Timeline | Group::ProviderCatalog => {
            let params = std::mem::take(&mut context.request.params);
            let result = agent_execution::dispatch(&context.request.method, params, state).await;
            context.respond(result)
        }
    }?;
    Ok(Completion::Complete)
}
