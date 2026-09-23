use std::collections::BTreeMap;
use std::sync::OnceLock;

use serde_json::{Value, json};
use server_protocol::checkout::CheckoutDiffUnsubscribeRequest;
use server_protocol::methods::{InboundKind, PASEO_METHODS};
use server_protocol::subscription::{SubscriptionReleaseRequest, SubscriptionReleaseResult};
use server_protocol::{ErrorCode, valid_id};
use uuid::Uuid;

use super::{ConnectionSubscriptions, MAX_SUBSCRIPTIONS};
use crate::Shared;
use crate::outbound::Outbound;

/// Business or connection-owned destination selected by a complete method name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Handler {
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
    Files,
    Base,
}

/// Inbound method metadata stored at one route-tree leaf.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Route {
    /// Accepted envelope direction.
    pub(super) kind: InboundKind,
    /// Capability the client must negotiate before using this method.
    pub(super) capability: &'static str,
    /// None for a catalog method whose business behavior is still a placeholder.
    pub(super) handler: Option<Handler>,
}

#[derive(Default)]
struct RouteNode {
    children: BTreeMap<&'static str, RouteNode>,
    route: Option<Route>,
}

impl RouteNode {
    fn leaf_mut(&mut self, method: &'static str) -> &mut Option<Route> {
        let mut node = self;
        for segment in method.split('.') {
            node = node.children.entry(segment).or_default();
        }
        &mut node.route
    }

    fn find(&self, method: &str) -> Option<&Route> {
        let mut node = self;
        for segment in method.split('.') {
            node = node.children.get(segment)?;
        }
        node.route.as_ref()
    }
}

fn routes() -> &'static RouteNode {
    static ROUTES: OnceLock<RouteNode> = OnceLock::new();
    ROUTES.get_or_init(build_routes)
}

fn build_routes() -> RouteNode {
    let mut root = RouteNode::default();
    for spec in PASEO_METHODS {
        let leaf = root.leaf_mut(spec.canonical_name);
        if let Some(existing) = leaf {
            assert_eq!(existing.kind, spec.kind, "conflicting method direction");
        } else {
            *leaf = Some(Route {
                kind: spec.kind,
                capability: spec.canonical_name,
                handler: None,
            });
        }
    }
    register_group(&mut root, Handler::Base, server_protocol::CAPABILITIES);
    register_group(
        &mut root,
        Handler::Projects,
        server_protocol::project_lease::CAPABILITIES,
    );
    register_group(
        &mut root,
        Handler::Agents,
        server_protocol::agent::CAPABILITIES,
    );
    register_group(
        &mut root,
        Handler::AgentRuntime,
        server_protocol::agent_lifecycle::CAPABILITIES,
    );
    register_group(
        &mut root,
        Handler::Directory,
        server_protocol::directory::CAPABILITIES,
    );
    register_group(
        &mut root,
        Handler::Directory,
        server_protocol::project_config::CAPABILITIES,
    );
    register_group(
        &mut root,
        Handler::Directory,
        server_protocol::project_icon::CAPABILITIES,
    );
    register_group(
        &mut root,
        Handler::Daemon,
        server_protocol::daemon::CAPABILITIES,
    );
    register_group(
        &mut root,
        Handler::Labels,
        server_protocol::workspace_labels::CAPABILITIES,
    );
    register_group(
        &mut root,
        Handler::Checkout,
        server_protocol::checkout::CAPABILITIES,
    );
    register_group(
        &mut root,
        Handler::Forge,
        server_protocol::forge::CAPABILITIES,
    );
    register_group(
        &mut root,
        Handler::Worktrees,
        server_protocol::worktrees::CAPABILITIES,
    );
    register_group(
        &mut root,
        Handler::Automation,
        server_protocol::workspace_automation::CAPABILITIES,
    );
    register_group(
        &mut root,
        Handler::WorkspaceState,
        server_protocol::workspace_state::CAPABILITIES,
    );
    register_group(
        &mut root,
        Handler::Files,
        server_protocol::files::CAPABILITIES,
    );
    let leaf = root.leaf_mut("server.status.unsubscribe");
    assert!(leaf.is_none(), "duplicate status unsubscribe route");
    *leaf = Some(Route {
        kind: InboundKind::Request,
        capability: "server.status.subscribe",
        handler: Some(Handler::Base),
    });
    root
}

fn register_group(root: &mut RouteNode, handler: Handler, methods: &'static [&'static str]) {
    for &method in methods {
        let leaf = root.leaf_mut(method);
        let route = leaf.get_or_insert(Route {
            kind: InboundKind::Request,
            capability: method,
            handler: None,
        });
        assert_eq!(
            route.kind,
            InboundKind::Request,
            "non-request handler route"
        );
        assert!(route.handler.is_none(), "duplicate request handler route");
        route.handler = Some(handler);
    }
}

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

/// Look up one exact method after following its dotted prefix nodes.
///
/// Returns `None` for unknown names and an unimplemented leaf for known placeholders.
pub(super) fn lookup(method: &str) -> Option<Route> {
    routes().find(method).copied()
}

/// Classify an event or client response by catalog direction and negotiated capability.
///
/// All current event and client response methods return a non-retryable placeholder error.
pub(super) fn placeholder(method: &str, kind: InboundKind, negotiated: &[String]) -> ErrorCode {
    let Some(route) = lookup(method) else {
        return ErrorCode::MethodNotFound;
    };
    if route.kind != kind {
        return ErrorCode::InvalidMessage;
    }
    if !negotiated
        .iter()
        .any(|capability| capability == route.capability)
    {
        return ErrorCode::UnsupportedCapability;
    }
    ErrorCode::NotImplemented
}

/// Dispatch an admitted, implemented request and retain post-response side effects.
pub(super) async fn route_request(
    handler: Handler,
    method: &str,
    params: Value,
    state: &Shared,
    outbound: &Outbound,
    subscriptions: &mut ConnectionSubscriptions,
) -> RouteResult {
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
        Handler::Files => RouteResult::plain(Err(ErrorCode::MethodNotFound)),
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
