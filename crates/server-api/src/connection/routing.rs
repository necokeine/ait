use std::collections::BTreeMap;
use std::sync::OnceLock;

use server_metadata::capabilities::Group as Metadata;
use server_protocol::ErrorCode;
use server_protocol::methods::{InboundKind, PASEO_METHODS};

pub(super) use crate::capabilities::Group as Handler;
use crate::capabilities::implemented_groups;

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
    for (handler, methods) in implemented_groups() {
        register_group(&mut root, handler, methods);
    }
    let leaf = root.leaf_mut("server.status.unsubscribe");
    assert!(leaf.is_none(), "duplicate status unsubscribe route");
    *leaf = Some(Route {
        kind: InboundKind::Request,
        capability: "server.status.subscribe",
        handler: Some(Handler::Metadata(Metadata::Base)),
    });
    root
}

fn register_group(root: &mut RouteNode, handler: Handler, methods: &'static [&'static str]) {
    for &method in methods {
        let leaf = root.leaf_mut(method);
        // Catalog methods retain their request/event direction; additional methods are requests.
        let route = leaf.get_or_insert(Route {
            kind: InboundKind::Request,
            capability: method,
            handler: None,
        });
        assert!(route.handler.is_none(), "duplicate handler route");
        route.handler = Some(handler);
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

#[cfg(test)]
mod tests;
