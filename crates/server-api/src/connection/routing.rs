use serde_json::{Value, json};
use server_protocol::checkout::CheckoutDiffUnsubscribeRequest;
use server_protocol::methods::{InboundKind, by_canonical_name};
use server_protocol::subscription::{SubscriptionReleaseRequest, SubscriptionReleaseResult};
use server_protocol::{ErrorCode, valid_id};
use uuid::Uuid;

use super::{ConnectionSubscriptions, MAX_SUBSCRIPTIONS};
use crate::Shared;
use crate::outbound::Outbound;

#[derive(Clone, Copy)]
enum Handler {
    Projects,
    Agents,
    AgentRuntime,
    Directory,
    Daemon,
    Labels,
    Checkout,
    Forge,
    Worktrees,
    Automation,
    WorkspaceState,
    Base,
}

const ROUTES: &[(Handler, &[&str])] = &[
    (
        Handler::Projects,
        server_protocol::project_lease::CAPABILITIES,
    ),
    (Handler::Agents, server_protocol::agent::CAPABILITIES),
    (
        Handler::AgentRuntime,
        server_protocol::agent_lifecycle::CAPABILITIES,
    ),
    (Handler::Directory, server_protocol::directory::CAPABILITIES),
    (
        Handler::Directory,
        server_protocol::project_config::CAPABILITIES,
    ),
    (
        Handler::Directory,
        server_protocol::project_icon::CAPABILITIES,
    ),
    (Handler::Daemon, server_protocol::daemon::CAPABILITIES),
    (
        Handler::Labels,
        server_protocol::workspace_labels::CAPABILITIES,
    ),
    (Handler::Checkout, server_protocol::checkout::CAPABILITIES),
    (Handler::Forge, server_protocol::forge::CAPABILITIES),
    (Handler::Worktrees, server_protocol::worktrees::CAPABILITIES),
    (
        Handler::Automation,
        server_protocol::workspace_automation::CAPABILITIES,
    ),
    (
        Handler::WorkspaceState,
        server_protocol::workspace_state::CAPABILITIES,
    ),
];

/// A routed request's value and subscriptions or events activated after its response is sent.
pub(super) struct RouteResult {
    /// Business result or a safe protocol error.
    pub(super) result: Result<Value, ErrorCode>,
    /// Label listener awaiting response admission.
    pub(super) label_subscription: Option<crate::workspace_labels::PendingSubscription>,
    /// Checkout listener awaiting response admission.
    pub(super) diff_subscription: Option<crate::checkout::PendingSubscription>,
    /// Workspace update emitted after the response.
    pub(super) workspace_event: Option<Value>,
}

impl RouteResult {
    fn plain(result: Result<Value, ErrorCode>) -> Self {
        Self {
            result,
            label_subscription: None,
            diff_subscription: None,
            workspace_event: None,
        }
    }
}

/// Return the capability required by a request method, or `None` for an unknown or wrong-kind name.
pub(super) fn request_capability(method: &str) -> Option<&str> {
    match method {
        "server.info"
        | "connection.ping"
        | "server.status.subscribe"
        | "subscription.release.request" => Some(method),
        "server.status.unsubscribe" => Some("server.status.subscribe"),
        _ if ROUTES.iter().any(|(_, methods)| methods.contains(&method)) => Some(method),
        _ => by_canonical_name(method)
            .filter(|spec| spec.kind == InboundKind::Request)
            .map(|spec| spec.canonical_name),
    }
}

/// Classify an event or client response by catalog direction and negotiated capability.
///
/// All current event and client response methods return a non-retryable placeholder error.
pub(super) fn placeholder(method: &str, kind: InboundKind, negotiated: &[String]) -> ErrorCode {
    let Some(spec) = by_canonical_name(method) else {
        return ErrorCode::MethodNotFound;
    };
    if spec.kind != kind {
        return ErrorCode::InvalidMessage;
    }
    if !negotiated.iter().any(|capability| capability == method) {
        return ErrorCode::UnsupportedCapability;
    }
    ErrorCode::NotImplemented
}

/// Dispatch an admitted, implemented request and retain post-response side effects.
pub(super) async fn route_request(
    method: &str,
    params: Value,
    state: &Shared,
    outbound: &Outbound,
    subscriptions: &mut ConnectionSubscriptions,
) -> RouteResult {
    let handler = ROUTES
        .iter()
        .find(|(_, methods)| methods.contains(&method))
        .map_or(Handler::Base, |(handler, _)| *handler);
    match handler {
        Handler::Labels => route_labels(method, params, state, outbound, subscriptions).await,
        Handler::Checkout => route_checkout(method, params, state, outbound, subscriptions).await,
        Handler::Worktrees => {
            let result = crate::worktrees::dispatch(method, params, state).await;
            match result {
                Ok(dispatched) => RouteResult {
                    result: Ok(dispatched.value),
                    label_subscription: None,
                    diff_subscription: None,
                    workspace_event: dispatched.event,
                },
                Err(error) => RouteResult::plain(Err(error)),
            }
        }
        Handler::WorkspaceState => {
            let result = crate::workspace_state::dispatch(method, params, state).await;
            match result {
                Ok(dispatched) => RouteResult {
                    result: Ok(dispatched.value),
                    label_subscription: None,
                    diff_subscription: None,
                    workspace_event: dispatched.event,
                },
                Err(error) => RouteResult::plain(Err(error)),
            }
        }
        Handler::Projects => {
            RouteResult::plain(crate::projects::dispatch(method, params, state).await)
        }
        Handler::Agents => RouteResult::plain(crate::agents::dispatch(method, params, state).await),
        Handler::AgentRuntime => {
            RouteResult::plain(crate::agent_runtime::dispatch(method, params, state).await)
        }
        Handler::Directory => {
            RouteResult::plain(crate::directory::dispatch(method, params, state).await)
        }
        Handler::Daemon => RouteResult::plain(crate::daemon::dispatch(method, params, state).await),
        Handler::Forge => RouteResult::plain(crate::forge::dispatch(method, params, state).await),
        Handler::Automation => {
            RouteResult::plain(crate::workspace_automation::dispatch(method, params, state).await)
        }
        Handler::Base => RouteResult::plain(dispatch_base(method, &params, state, subscriptions)),
    }
}

async fn route_labels(
    method: &str,
    params: Value,
    state: &Shared,
    outbound: &Outbound,
    subscriptions: &mut ConnectionSubscriptions,
) -> RouteResult {
    if method == "workspace.label.list.request"
        && params
            .get("subscribe")
            .is_some_and(|subscribe| !subscribe.is_null())
        && subscriptions.len() >= MAX_SUBSCRIPTIONS
    {
        return RouteResult::plain(Err(ErrorCode::ResourceExhausted));
    }
    match crate::workspace_labels::dispatch(method, params, state, outbound.clone()).await {
        Ok(dispatched) => RouteResult {
            result: Ok(dispatched.value),
            label_subscription: dispatched.subscription,
            diff_subscription: None,
            workspace_event: None,
        },
        Err(error) => RouteResult::plain(Err(error)),
    }
}

async fn route_checkout(
    method: &str,
    params: Value,
    state: &Shared,
    outbound: &Outbound,
    subscriptions: &mut ConnectionSubscriptions,
) -> RouteResult {
    if method == "checkout.diff.unsubscribe.request" {
        let request = serde_json::from_value::<CheckoutDiffUnsubscribeRequest>(params);
        let request = match request {
            Ok(request) if valid_id(&request.subscription_id) => request,
            _ => return RouteResult::plain(Err(ErrorCode::InvalidMessage)),
        };
        if subscriptions
            .diffs
            .remove(&request.subscription_id)
            .is_none()
        {
            return RouteResult::plain(Err(ErrorCode::SubscriptionNotFound));
        }
        return RouteResult::plain(Ok(json!({"subscriptionId":request.subscription_id})));
    }
    if method == "checkout.diff.subscribe.request" {
        let replacement = params
            .get("subscriptionId")
            .and_then(Value::as_str)
            .is_some_and(|id| subscriptions.diffs.contains_key(id));
        if subscriptions.len() >= MAX_SUBSCRIPTIONS && !replacement {
            return RouteResult::plain(Err(ErrorCode::ResourceExhausted));
        }
    }
    match crate::checkout::dispatch(method, params, state, outbound.clone()).await {
        Ok(dispatched) => RouteResult {
            result: Ok(dispatched.value),
            label_subscription: None,
            diff_subscription: dispatched.subscription,
            workspace_event: None,
        },
        Err(error) => RouteResult::plain(Err(error)),
    }
}

fn dispatch_base(
    method: &str,
    params: &Value,
    state: &Shared,
    subscriptions: &mut ConnectionSubscriptions,
) -> Result<Value, ErrorCode> {
    match method {
        "server.info" => serde_json::to_value(state.info()).map_err(|_| ErrorCode::InvalidMessage),
        "connection.ping" => {
            let nonce = params
                .get("nonce")
                .and_then(Value::as_str)
                .filter(|nonce| valid_id(nonce))
                .ok_or(ErrorCode::InvalidMessage)?;
            Ok(json!({"nonce":nonce}))
        }
        "server.status.subscribe" => {
            if subscriptions.len() >= MAX_SUBSCRIPTIONS {
                return Err(ErrorCode::ResourceExhausted);
            }
            let id = Uuid::new_v4().to_string();
            subscriptions.status.insert(id.clone());
            Ok(json!({"subscription_id":id}))
        }
        "server.status.unsubscribe" => {
            let id = params
                .get("subscription_id")
                .and_then(Value::as_str)
                .ok_or(ErrorCode::InvalidMessage)?;
            if !subscriptions.status.remove(id) {
                return Err(ErrorCode::SubscriptionNotFound);
            }
            Ok(json!({"unsubscribed":true}))
        }
        "subscription.release.request" => {
            let request: SubscriptionReleaseRequest =
                serde_json::from_value(params.clone()).map_err(|_| ErrorCode::InvalidMessage)?;
            if !valid_id(&request.subscription_id) {
                return Err(ErrorCode::InvalidMessage);
            }
            subscriptions.status.remove(&request.subscription_id);
            subscriptions.labels.remove(&request.subscription_id);
            subscriptions.diffs.remove(&request.subscription_id);
            subscriptions.files.release(&request.subscription_id);
            serde_json::to_value(SubscriptionReleaseResult {
                subscription_id: request.subscription_id,
            })
            .map_err(|_| ErrorCode::InvalidMessage)
        }
        _ => Err(ErrorCode::MethodNotFound),
    }
}

#[cfg(test)]
mod tests;
