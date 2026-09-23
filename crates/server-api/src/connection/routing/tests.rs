use std::collections::BTreeSet;

use super::*;

#[test]
fn every_implemented_request_has_one_dispatch_route_or_connection_handler() {
    let routes = ROUTES
        .iter()
        .flat_map(|(_, methods)| methods.iter().copied())
        .collect::<BTreeSet<_>>();
    let installed = crate::installed_capabilities(&crate::Services::default());
    for method in installed {
        if server_protocol::files::CAPABILITIES.contains(&method.as_str()) {
            continue;
        }
        assert!(
            routes.contains(method.as_str()) || request_capability(&method) == Some(&method),
            "missing dispatch route: {method}"
        );
    }
    assert_eq!(routes.len(), 89);
}

#[test]
fn catalog_placeholders_keep_direction_and_negotiation_checks() {
    let offered = [
        "schedule.list.request".to_owned(),
        "terminal.input".to_owned(),
    ];
    assert_eq!(
        request_capability("schedule.list.request"),
        Some("schedule.list.request")
    );
    assert_eq!(request_capability("terminal.input"), None);
    assert_eq!(request_capability("project.open"), Some("project.open"));
    assert_eq!(request_capability("schedule/list"), None);
    assert_eq!(
        placeholder("terminal.input", InboundKind::Event, &offered),
        ErrorCode::NotImplemented
    );
    assert_eq!(
        placeholder("terminal.input", InboundKind::Response, &offered),
        ErrorCode::InvalidMessage
    );
    assert_eq!(
        placeholder("push.register", InboundKind::Event, &offered),
        ErrorCode::UnsupportedCapability
    );
    assert_eq!(
        placeholder("register_push_token", InboundKind::Event, &offered),
        ErrorCode::MethodNotFound
    );
}
