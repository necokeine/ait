use std::collections::BTreeSet;

use super::*;

fn handler(method: &str) -> Option<Handler> {
    lookup(method).and_then(|route| route.handler)
}

fn request_capability(method: &str) -> Option<&'static str> {
    lookup(method)
        .filter(|route| route.kind == InboundKind::Request)
        .map(|route| route.capability)
}

fn implemented_groups() -> &'static [(Handler, &'static [&'static str])] {
    &[
        (Handler::Base, server_protocol::CAPABILITIES),
        (
            Handler::Session,
            server_metadata::protocol::session::CAPABILITIES,
        ),
        (
            Handler::AgentExecution,
            server_provider::protocol::agent_config::CAPABILITIES,
        ),
        (
            Handler::AgentExecution,
            server_provider::protocol::agent_execution::CAPABILITIES,
        ),
        (
            Handler::Agents,
            server_provider::protocol::agent::CAPABILITIES,
        ),
        (
            Handler::AgentRuntime,
            server_provider::protocol::agent_lifecycle::CAPABILITIES,
        ),
        (
            Handler::Directory,
            server_metadata::protocol::directory::CAPABILITIES,
        ),
        (
            Handler::GithubProjects,
            server_filesystem::protocol::github_projects::CAPABILITIES,
        ),
        (
            Handler::Directory,
            server_metadata::protocol::project_config::CAPABILITIES,
        ),
        (
            Handler::Directory,
            server_metadata::protocol::project_icon::CAPABILITIES,
        ),
        (
            Handler::Daemon,
            server_metadata::protocol::daemon::CAPABILITIES,
        ),
        (
            Handler::Labels,
            server_metadata::protocol::workspace_labels::CAPABILITIES,
        ),
        (
            Handler::Checkout,
            server_filesystem::protocol::checkout::CAPABILITIES,
        ),
        (
            Handler::Forge,
            server_filesystem::protocol::forge::CAPABILITIES,
        ),
        (
            Handler::Worktrees,
            server_filesystem::protocol::worktrees::CAPABILITIES,
        ),
        (
            Handler::Automation,
            server_metadata::protocol::workspace_automation::CAPABILITIES,
        ),
        (
            Handler::WorkspaceRecovery,
            server_filesystem::protocol::workspace_recovery::CAPABILITIES,
        ),
        (
            Handler::WorkspaceState,
            server_metadata::protocol::workspace_state::CAPABILITIES,
        ),
        (
            Handler::Files,
            server_filesystem::protocol::files::CAPABILITIES,
        ),
    ]
}

#[test]
fn hierarchy_routes_every_implemented_method_to_exactly_one_handler() {
    let groups = implemented_groups();
    let mut implemented = BTreeSet::new();
    for &(expected, methods) in groups {
        for &method in methods {
            assert!(
                implemented.insert(method),
                "duplicate implementation: {method}"
            );
            let route = routes().find(method).expect("implemented route must exist");
            assert_eq!(route.kind, InboundKind::Request, "{method}");
            assert_eq!(route.capability, method, "{method}");
            assert_eq!(route.handler, Some(expected), "{method}");
        }
    }
    let heartbeat = server_metadata::protocol::server::HEARTBEAT_METHOD;
    implemented.insert(heartbeat);
    assert_eq!(lookup(heartbeat).unwrap().kind, InboundKind::Event);
    assert_eq!(handler(heartbeat), Some(Handler::Session));
    for &method in server_terminal::protocol::CAPABILITIES {
        assert!(implemented.insert(method));
        assert_eq!(handler(method), Some(Handler::Terminal));
        assert_eq!(
            lookup(method).unwrap().kind,
            if method == "terminal.input" {
                InboundKind::Event
            } else {
                InboundKind::Request
            }
        );
    }
    assert_eq!(implemented.len(), 122);

    let advertised = crate::registered_capabilities(
        &implemented
            .iter()
            .map(|method| (*method).to_owned())
            .collect::<Vec<_>>(),
    );
    assert_eq!(advertised.len(), 195);
    for method in &advertised {
        let route = routes().find(method).expect("advertised route must exist");
        assert_eq!(
            route.handler.is_some(),
            implemented.contains(method.as_str())
        );
    }
    for spec in PASEO_METHODS {
        let route = routes()
            .find(spec.canonical_name)
            .expect("catalog method must exist");
        assert_eq!(route.kind, spec.kind, "{}", spec.canonical_name);
    }
}

#[test]
fn prefix_nodes_can_be_methods_and_have_children_with_different_owners() {
    assert_eq!(handler("project.list"), None);
    assert_eq!(handler("project.list.request"), Some(Handler::Directory));
    assert_eq!(handler("workspace.open.request"), Some(Handler::Directory));
    assert_eq!(
        handler("workspace.github.search_repositories.request"),
        Some(Handler::GithubProjects)
    );
    assert_eq!(
        handler("workspace.label.list.request"),
        Some(Handler::Labels)
    );
    assert_eq!(
        handler("workspace.worktree.list.request"),
        Some(Handler::Worktrees)
    );
    assert_eq!(
        handler("checkout.diff.get.request"),
        Some(Handler::Checkout)
    );
    assert_eq!(handler("checkout.pr.create.request"), Some(Handler::Forge));
    assert_eq!(handler("file.upload.request"), Some(Handler::Files));
    assert_eq!(handler("schedule.list.request"), None);
    assert_eq!(routes().find("project.list.unknown"), None);
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
    assert_eq!(request_capability("project.open"), None);
    assert_eq!(
        request_capability("server.status.unsubscribe"),
        Some("server.status.subscribe")
    );
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
