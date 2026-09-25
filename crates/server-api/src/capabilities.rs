use server_filesystem::capabilities as filesystem;
use server_metadata::capabilities as metadata;
use server_provider::capabilities as provider;
use server_terminal::capabilities as terminal;

use crate::Services;

/// Crate-owned group whose transport handler is selected by the API.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Group {
    Metadata(metadata::Group),
    Filesystem(filesystem::Group),
    Provider(provider::Group),
    Terminal(terminal::Group),
}

/// Merge crate-owned declarations without copying their method lists.
pub(super) fn implemented_groups() -> impl Iterator<Item = (Group, &'static [&'static str])> {
    metadata::IMPLEMENTED_GROUPS
        .iter()
        .map(|&(group, methods)| (Group::Metadata(group), methods))
        .chain(
            filesystem::IMPLEMENTED_GROUPS
                .iter()
                .map(|&(group, methods)| (Group::Filesystem(group), methods)),
        )
        .chain(
            provider::IMPLEMENTED_GROUPS
                .iter()
                .map(|&(group, methods)| (Group::Provider(group), methods)),
        )
        .chain(
            terminal::IMPLEMENTED_GROUPS
                .iter()
                .map(|&(group, methods)| (Group::Terminal(group), methods)),
        )
}

/// Supply service presence to each owner and collect its installed method names.
pub(super) fn installed_capabilities(services: &Services) -> Vec<String> {
    metadata::installed_capabilities(metadata::InstalledServices {
        directory: services.directory.is_some(),
        daemon: services.daemon.is_some(),
        workspace_labels: services.workspace_labels.is_some(),
        workspace_automation: services.workspace_automation.is_some(),
        workspace_state: services.workspace_state.is_some(),
    })
    .chain(filesystem::installed_capabilities(
        filesystem::InstalledServices {
            checkout: services.checkout.is_some(),
            forge: services.forge.is_some(),
            files: services.files.is_some(),
            github_projects: services.github_projects.is_some(),
            worktrees: services.worktrees.is_some(),
            workspace_recovery: services.workspace_recovery.is_some(),
        },
    ))
    .chain(provider::installed_capabilities(
        provider::InstalledServices {
            agents: services.agents.is_some(),
            agent_runtime: services.agent_runtime.is_some(),
            agent_execution: services.agent_execution.is_some(),
        },
    ))
    .chain(terminal::installed_capabilities(
        terminal::InstalledServices {
            terminals: services.terminals.is_some(),
        },
    ))
    .map(str::to_owned)
    .collect()
}

#[cfg(test)]
mod tests;
