use std::collections::BTreeSet;

use super::*;

#[test]
fn every_installation_combination_advertises_only_available_services() {
    for mask in 0..128 {
        let services = InstalledServices {
            checkout: mask & 1 != 0,
            forge: mask & 2 != 0,
            files: mask & 4 != 0,
            github_projects: mask & 8 != 0,
            worktrees: mask & 16 != 0,
            workspace_recovery: mask & 32 != 0,
            skills: mask & 64 != 0,
        };
        let capabilities: Vec<_> = installed_capabilities(services).collect();
        let methods: BTreeSet<_> = capabilities.iter().copied().collect();
        let expected_count = 20 * usize::from(services.checkout)
            + 10 * usize::from(services.forge)
            + 11 * usize::from(services.files)
            + 2 * usize::from(services.github_projects)
            + 3 * usize::from(services.worktrees)
            + 2 * usize::from(services.workspace_recovery)
            + 5 * usize::from(services.skills);
        assert_eq!(
            capabilities.len(),
            methods.len(),
            "duplicate method for {mask}"
        );
        assert_eq!(methods.len(), expected_count, "installation {mask}");
        assert_eq!(
            methods.contains("checkout.status.get.request"),
            services.checkout
        );
        assert_eq!(methods.contains("forge.search.request"), services.forge);
        assert_eq!(
            methods.contains("directory.suggestions.request"),
            services.files
        );
        assert_eq!(methods.contains("file.upload.request"), services.files);
        assert_eq!(
            methods.contains("workspace.github.search_repositories.request"),
            services.github_projects
        );
        assert_eq!(
            methods.contains("workspace.worktree.list.request"),
            services.worktrees
        );
        assert_eq!(
            methods.contains("workspace.recovery.inspect.request"),
            services.workspace_recovery
        );
        assert_eq!(
            methods.contains("agent.skills.get_status.request"),
            services.skills
        );
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
    assert_eq!(declared.len(), 53);
    let installed: Vec<_> = installed_capabilities(InstalledServices {
        checkout: true,
        forge: true,
        files: true,
        github_projects: true,
        worktrees: true,
        workspace_recovery: true,
        skills: true,
    })
    .collect();
    assert_eq!(installed, declared);
}
