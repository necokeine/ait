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
    /// Installed terminals service.
    pub terminals: Option<Arc<Mutex<crate::service::Terminals>>>,
}

impl std::ops::Deref for State {
    type Target = Runtime;

    fn deref(&self) -> &Runtime {
        &self.runtime
    }
}

/// Dispatch an admitted terminal request using its connection-owned streams.
/// # Errors
/// Returns a delivery failure; terminal errors are sent as protocol responses.
pub async fn dispatch(
    group: Group,
    context: Context<'_>,
    state: &State,
    connection: &mut crate::connection::TerminalConnection,
) -> Result<(), QueueError> {
    match group {
        Group::Terminal => {
            connection
                .request(
                    crate::connection::Request {
                        id: context.request.id,
                        method: context.request.method,
                        params: context.request.params,
                        available: context.available_subscriptions,
                    },
                    state,
                    context.outbound,
                )
                .await
        }
    }
}

/// Finish an admitted cross-capability close after the provider has closed its Agents.
/// # Errors
/// Returns terminal task admission or I/O errors, preserving per-terminal success results.
pub async fn close_many(state: &State, ids: Vec<String>) -> Result<serde_json::Value, ErrorCode> {
    crate::connection::run(state, move |terminals| {
        Ok(serde_json::json!(
            ids.into_iter()
                .map(|id| {
                    let success = terminals.kill(&id).is_ok();
                    serde_json::json!({"terminalId":id,"success":success})
                })
                .collect::<Vec<_>>()
        ))
    })
    .await
}
