//! Connection-owned Timeline observers and plugin display provenance.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::json;
use server_model::events::Subscription;
use server_model::outbound::QueueError;
use server_model::{Context, ErrorCode};
use uuid::Uuid;

use crate::dispatch::State;
use crate::protocol::timeline::SubscriptionRequest;

/// Provider connection state. The diagnostic client label never grants additional authority.
#[derive(Debug, Default)]
pub struct Connection {
    subscriptions: BTreeMap<String, Subscription>,
    plugin: Option<String>,
}

impl Connection {
    /// Capture plugin item provenance from the trusted-token client's diagnostic label.
    /// This is a namespace convention, not a separately authenticated plugin principal.
    #[must_use]
    pub fn new(client_id: &str) -> Self {
        let plugin = client_id
            .strip_prefix("plugin:")
            .filter(|id| {
                !id.is_empty()
                    && id.len() <= 128
                    && id
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
            })
            .map(str::to_owned);
        Self {
            plugin,
            ..Self::default()
        }
    }

    /// Count active observers toward the transport's shared subscription budget.
    #[must_use]
    pub fn len(&self) -> usize {
        self.subscriptions.len()
    }

    /// Whether no Provider observers remain.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.subscriptions.is_empty()
    }

    /// Stop one observer only if it belongs to this physical connection.
    pub fn release(&mut self, id: &str) {
        self.subscriptions.remove(id);
    }

    pub(crate) fn plugin(&self) -> Option<&str> {
        self.plugin.as_deref()
    }

    pub(crate) async fn create(
        &mut self,
        mut context: Context<'_>,
        state: &State,
    ) -> Result<(), QueueError> {
        if !context.request.params.is_object() {
            return context.respond(Err(ErrorCode::InvalidMessage));
        }
        let observe = context
            .request
            .params
            .get("subscribe")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        if observe && context.available_subscriptions == 0 {
            return context.respond(Err(ErrorCode::ResourceExhausted));
        }
        let key = match context.request.params.get("idempotencyKey") {
            Some(serde_json::Value::String(key)) => key.clone(),
            None | Some(serde_json::Value::Null) => Uuid::new_v4().to_string(),
            _ => return context.respond(Err(ErrorCode::InvalidMessage)),
        };
        context.request.params["idempotencyKey"] = json!(key);
        let pending = if observe {
            let Some(execution) = &state.agent_execution else {
                return context.respond(Err(ErrorCode::UnsupportedCapability));
            };
            let creations = execution.creations();
            let outbound = context.outbound.clone();
            let result = context
                .runtime
                .run(
                    Some(std::sync::Arc::new(std::sync::Mutex::new(creations))),
                    ErrorCode::RegistryIo,
                    move |creations| {
                        creations.subscribe(
                            server_metadata::protocol::creation::Kind::Agent,
                            &key,
                            outbound,
                        )
                    },
                )
                .await;
            match result {
                Ok((_, subscription)) => Some(subscription),
                Err(error) => return context.respond(Err(error)),
            }
        } else {
            None
        };
        let result = super::dispatch::agent_execution::dispatch(
            "agent.create.request",
            std::mem::take(&mut context.request.params),
            state,
        )
        .await;
        match result {
            Ok(mut value) => {
                if let Some(subscription) = &pending {
                    value["subscriptionId"] = json!(subscription.id());
                }
                context.respond(Ok(value))?;
                if let Some(subscription) = pending {
                    subscription.activate()?;
                    self.subscriptions
                        .insert(subscription.id().to_owned(), subscription);
                }
                Ok(())
            }
            Err(error) => context.respond(Err(error)),
        }
    }

    pub(crate) async fn subscribe(
        &mut self,
        context: Context<'_>,
        state: &State,
    ) -> Result<(), QueueError> {
        let prepared = async {
            let request: SubscriptionRequest =
                serde_json::from_value(context.request.params.clone())
                    .map_err(|_| ErrorCode::InvalidMessage)?;
            if request.agent_ids.len() > 32 {
                return Err(ErrorCode::InvalidMessage);
            }
            if context.available_subscriptions == 0 {
                return Err(ErrorCode::ResourceExhausted);
            }
            let execution = state
                .agent_execution
                .as_ref()
                .ok_or(ErrorCode::UnsupportedCapability)?;
            let mut ids = BTreeSet::new();
            for id in request.agent_ids {
                let value = execution
                    .execute("agent.get.request", json!({"agentId":id}))
                    .await?;
                let resolved = value
                    .pointer("/agent/id")
                    .and_then(serde_json::Value::as_str)
                    .ok_or(ErrorCode::AgentNotFound)?;
                ids.insert(resolved.to_owned());
            }
            let subscription = execution.timeline().events().subscribe(
                Uuid::new_v4().to_string(),
                ids.clone(),
                context.outbound.clone(),
            );
            Ok((ids, subscription))
        }
        .await;
        match prepared {
            Ok((ids, subscription)) => {
                let id = subscription.id().to_owned();
                context.respond(Ok(json!({"subscriptionId":id,"agentIds":ids})))?;
                subscription.activate()?;
                self.subscriptions.insert(id, subscription);
                Ok(())
            }
            Err(error) => context.respond(Err(error)),
        }
    }
}
