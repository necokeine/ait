use std::collections::BTreeSet;

use super::*;

#[test]
fn every_installation_combination_advertises_only_available_services() {
    for mask in 0..32 {
        let services = InstalledServices {
            directory: mask & 1 != 0,
            daemon: mask & 2 != 0,
            workspace_labels: mask & 4 != 0,
            workspace_automation: mask & 8 != 0,
            workspace_state: mask & 16 != 0,
        };
        let capabilities: Vec<_> = installed_capabilities(services).collect();
        let methods: BTreeSet<_> = capabilities.iter().copied().collect();
        let expected_count = 6
            + 15 * usize::from(services.directory)
            + 9 * usize::from(services.daemon)
            + 5 * usize::from(services.workspace_labels)
            + 5 * usize::from(services.workspace_automation)
            + 2 * usize::from(services.workspace_state);
        assert_eq!(
            capabilities.len(),
            methods.len(),
            "duplicate method for {mask}"
        );
        assert_eq!(methods.len(), expected_count, "installation {mask}");
        assert_eq!(methods.contains("project.list.request"), services.directory);
        assert_eq!(
            methods.contains("project.config.read.request"),
            services.directory
        );
        assert_eq!(
            methods.contains("project.icon.get.request"),
            services.directory
        );
        assert_eq!(
            methods.contains("daemon.get_status.request"),
            services.daemon
        );
        assert_eq!(
            methods.contains("workspace.label.list.request"),
            services.workspace_labels
        );
        assert_eq!(
            methods.contains("workspace.setup.status.request"),
            services.workspace_automation
        );
        assert_eq!(
            methods.contains("workspace.mark_unread.request"),
            services.workspace_state
        );
        assert!(methods.contains("server.info"));
        assert!(methods.contains("subscription.release.request"));
        assert!(methods.contains("session.heartbeat"));
        assert!(methods.contains("session.events.set_subscription.request"));
        assert!(!methods.contains("server.status.unsubscribe"));
    }
}

#[test]
fn implemented_groups_are_unique_and_match_a_full_installation() {
    let declared: Vec<_> = IMPLEMENTED_GROUPS
        .iter()
        .flat_map(|(_, methods)| methods.iter().copied())
        .collect();
    let unique: BTreeSet<_> = declared.iter().copied().collect();
    assert_eq!(declared.len(), unique.len());
    assert_eq!(declared.len(), 42);
    let installed: Vec<_> = installed_capabilities(InstalledServices {
        directory: true,
        daemon: true,
        workspace_labels: true,
        workspace_automation: true,
        workspace_state: true,
    })
    .collect();
    assert_eq!(installed, declared);
}
