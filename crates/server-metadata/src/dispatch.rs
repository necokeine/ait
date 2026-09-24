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
    /// Installed daemon service.
    pub daemon: Option<Arc<Mutex<crate::service::daemon::Daemon>>>,
    /// Installed directory service.
    pub directory: Option<Arc<Mutex<crate::service::directory::Directory>>>,
    /// Installed workspace labels service.
    pub workspace_labels: Option<Arc<Mutex<crate::service::workspace_labels::WorkspaceLabels>>>,
    /// Installed workspace automation service.
    pub workspace_automation:
        Option<Arc<Mutex<crate::service::workspace_automation::WorkspaceAutomation>>>,
    /// Installed workspace state service.
    pub workspace_state: Option<Arc<Mutex<crate::service::workspace_state::WorkspaceState>>>,
    /// Shared ephemeral session event hub.
    pub session_events: crate::service::session::SessionEvents,
    /// Whether Agent attention event production is installed.
    pub has_agent_execution: bool,
}

impl std::ops::Deref for State {
    type Target = Runtime;

    fn deref(&self) -> &Runtime {
        &self.runtime
    }
}

use serde_json::{Value, json};
use server_model::subscription::SubscriptionReleaseRequest;
use server_model::{Lifecycle, LifecycleIntent, ServerMessage, valid_id};
use uuid::Uuid;

use crate::connection::Connection;

/// Connection-wide work that follows metadata dispatch.
pub enum Completion {
    /// The response has been delivered by metadata.
    Complete,
    /// Release this connection's matching observers in every capability crate, then acknowledge.
    Release {
        /// Correlation ID of the admitted release request.
        request_id: String,
        /// Validated connection-local subscription ID.
        subscription_id: String,
    },
}

impl State {
    /// Request a process lifecycle transition under the common admission lock.
    pub fn request_lifecycle(&self, intent: LifecycleIntent) {
        let admission = self
            .admission
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut requested = self
            .lifecycle_intent
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if requested.is_none() {
            *requested = Some(intent);
        }
        self.start_draining();
        drop(requested);
        drop(admission);
    }

    /// Publish the first draining transition and close task admission.
    pub fn start_draining(&self) {
        if !self.cancellation.is_cancelled() {
            let mut info = self.info();
            info.lifecycle = Lifecycle::Draining;
            self.session_events.publish(
                crate::protocol::session::SessionEventKind::ServerInfo,
                &json!({"status":"server_info","info":info}),
            );
        }
        self.cancellation.cancel();
        self.tasks.close();
    }
}

/// Dispatch an admitted request using concrete context, services and connection state.
/// # Errors
/// Returns a delivery failure; business errors are sent using the request's correlation ID.
pub async fn dispatch(
    group: Group,
    mut context: Context<'_>,
    state: &State,
    connection: &mut Connection,
) -> Result<Completion, QueueError> {
    match group {
        Group::Base => return base(context, state, connection),
        Group::Session => crate::connection::session::subscribe(context, state, connection),
        Group::Directory => {
            context
                .rpc(
                    state.directory.clone(),
                    ErrorCode::RegistryIo,
                    crate::rpc::directory::execute,
                )
                .await
        }
        Group::Daemon => {
            let params = std::mem::take(&mut context.request.params);
            let result =
                crate::connection::daemon::dispatch(&context.request.method, params, state).await;
            context.respond(result)
        }
        Group::Labels => labels(context, state, connection).await,
        Group::Automation => {
            context
                .rpc(
                    state.workspace_automation.clone(),
                    ErrorCode::RegistryIo,
                    |service, method, params| {
                        crate::rpc::workspace_automation::execute(service, method, params)
                    },
                )
                .await
        }
        Group::WorkspaceState => {
            let result = context
                .call(
                    state.workspace_state.clone(),
                    ErrorCode::RegistryIo,
                    crate::rpc::workspace_state::execute,
                )
                .await;
            match result {
                Ok(reply) => context.workspace(reply.value, reply.event),
                Err(error) => context.respond(Err(error)),
            }
        }
    }?;
    Ok(Completion::Complete)
}

async fn labels(
    mut context: Context<'_>,
    state: &State,
    connection: &mut Connection,
) -> Result<(), QueueError> {
    if context.request.method == "workspace.label.list.request"
        && context
            .request
            .params
            .get("subscribe")
            .is_some_and(|value| !value.is_null())
        && context.available_subscriptions == 0
    {
        return context.respond(Err(ErrorCode::ResourceExhausted));
    }
    let params = std::mem::take(&mut context.request.params);
    let reply = crate::connection::workspace_labels::dispatch(
        &context.request.method,
        params,
        state,
        context.outbound.clone(),
    )
    .await;
    match reply {
        Ok(reply) => {
            context.respond(Ok(reply.value))?;
            if let Some(pending) = reply.subscription {
                let (id, subscription) = pending.activate().map_err(|error| match error {
                    crate::rpc::workspace_labels::DeliveryError::Encode(error) => {
                        QueueError::Encode(error)
                    }
                    crate::rpc::workspace_labels::DeliveryError::Closed => QueueError::Full,
                })?;
                connection.labels.insert(id, subscription);
            }
            Ok(())
        }
        Err(error) => context.respond(Err(error)),
    }
}

fn base(
    context: Context<'_>,
    state: &State,
    connection: &mut Connection,
) -> Result<Completion, QueueError> {
    if context.request.method == "subscription.release.request" {
        let request =
            serde_json::from_value::<SubscriptionReleaseRequest>(context.request.params.clone());
        if let Ok(request) = request
            && valid_id(&request.subscription_id)
        {
            return Ok(Completion::Release {
                request_id: context.request.id,
                subscription_id: request.subscription_id,
            });
        }
        context.respond(Err(ErrorCode::InvalidMessage))?;
        return Ok(Completion::Complete);
    }
    let mut status = None;
    let result = (|| {
        Ok(match context.request.method.as_str() {
            "server.info" => {
                serde_json::to_value(state.info()).map_err(|_| ErrorCode::InvalidMessage)?
            }
            "connection.ping" => crate::rpc::server::ping(context.request.params.clone())?,
            "server.status.subscribe" => {
                if context.available_subscriptions == 0 {
                    return Err(ErrorCode::ResourceExhausted);
                }
                let id = Uuid::new_v4().to_string();
                connection.status.insert(id.clone());
                status = Some(id.clone());
                json!({"subscription_id":id})
            }
            "server.status.unsubscribe" => {
                let id = context
                    .request
                    .params
                    .get("subscription_id")
                    .and_then(Value::as_str)
                    .ok_or(ErrorCode::InvalidMessage)?;
                if !connection.status.remove(id) {
                    return Err(ErrorCode::SubscriptionNotFound);
                }
                json!({"unsubscribed":true})
            }
            _ => return Err(ErrorCode::MethodNotFound),
        })
    })();
    let outbound = context.outbound;
    context.respond(result)?;
    if let Some(subscription_id) = status {
        outbound.send(&ServerMessage::Status {
            subscription_id,
            lifecycle: state.info().lifecycle,
        })?;
    }
    Ok(Completion::Complete)
}
