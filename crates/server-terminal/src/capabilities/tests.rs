use std::collections::BTreeSet;

use super::*;

#[test]
fn every_installation_combination_advertises_only_available_services() {
    for mask in 0..2 {
        let services = InstalledServices {
            terminals: mask & 1 != 0,
        };
        let capabilities: Vec<_> = installed_capabilities(services).collect();
        let methods: BTreeSet<_> = capabilities.iter().copied().collect();
        let expected_count = 10 * usize::from(services.terminals);
        assert_eq!(
            capabilities.len(),
            methods.len(),
            "duplicate method for {mask}"
        );
        assert_eq!(methods.len(), expected_count, "installation {mask}");
        assert_eq!(
            methods.contains("terminal.list.request"),
            services.terminals
        );
        assert_eq!(methods.contains("terminal.input"), services.terminals);
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
    assert_eq!(declared.len(), 10);
    let installed: Vec<_> = installed_capabilities(InstalledServices { terminals: true }).collect();
    assert_eq!(installed, declared);
}
