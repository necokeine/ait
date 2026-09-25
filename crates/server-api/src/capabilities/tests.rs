use std::collections::BTreeSet;

use super::*;

#[test]
fn empty_host_keeps_only_builtin_metadata_methods() {
    let methods = installed_capabilities(&Services::default());
    assert_eq!(
        methods,
        [
            "server.info",
            "connection.ping",
            "server.status.subscribe",
            "subscription.release.request",
            "session.heartbeat",
            "session.events.set_subscription.request",
        ]
    );
    let registered = crate::registered_capabilities(&methods);
    assert_eq!(registered.len(), 190);
    assert!(registered.iter().any(|method| method == "terminal.input"));
    assert!(!methods.iter().any(|method| method == "terminal.input"));
}

#[test]
fn merged_groups_have_one_owner_per_method_and_keep_placeholders_separate() {
    let methods: Vec<_> = implemented_groups()
        .flat_map(|(_, methods)| methods.iter().copied())
        .collect();
    let unique: BTreeSet<_> = methods.iter().copied().collect();
    assert_eq!(methods.len(), unique.len());
    assert_eq!(methods.len(), 122);
    assert!(!unique.contains("schedule.list.request"));
    assert!(!unique.contains("server.status.unsubscribe"));
    let registered =
        crate::registered_capabilities(&methods.into_iter().map(str::to_owned).collect::<Vec<_>>());
    assert_eq!(registered.len(), 195);
}
