use std::collections::BTreeSet;

use super::*;

#[test]
fn every_installation_combination_advertises_only_available_services() {
    for mask in 0..8 {
        let services = InstalledServices {
            agents: mask & 1 != 0,
            agent_runtime: mask & 2 != 0,
            agent_execution: mask & 4 != 0,
        };
        let capabilities: Vec<_> = installed_capabilities(services).collect();
        let methods: BTreeSet<_> = capabilities.iter().copied().collect();
        let expected_count = 5 * usize::from(services.agents)
            + 9 * usize::from(services.agent_runtime || services.agent_execution)
            + 32 * usize::from(services.agent_execution);
        assert_eq!(
            capabilities.len(),
            methods.len(),
            "duplicate method for {mask}"
        );
        assert_eq!(methods.len(), expected_count, "installation {mask}");
        assert_eq!(methods.contains("agent.configure"), services.agents);
        assert_eq!(
            methods.contains("agent.list.request"),
            services.agent_runtime || services.agent_execution
        );
        assert_eq!(
            methods.contains("agent.items.close.request"),
            services.agent_runtime || services.agent_execution
        );
        assert_eq!(
            methods.contains("agent.create.request"),
            services.agent_execution
        );
        assert_eq!(
            methods.contains("agent.finish.wait.request"),
            services.agent_execution
        );
        assert_eq!(
            methods.contains("agent.model.set.request"),
            services.agent_execution
        );
        assert!(!methods.contains("provider.list.request"));
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
    assert_eq!(declared.len(), 46);
    let installed: Vec<_> = installed_capabilities(InstalledServices {
        agents: true,
        agent_runtime: true,
        agent_execution: true,
    })
    .collect();
    assert_eq!(installed, declared);
}
