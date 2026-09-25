use server_browser::capabilities as browser;
use server_filesystem::capabilities as filesystem;
use server_metadata::capabilities as metadata;
use server_provider::capabilities as provider;
use server_schedule::capabilities as schedule;
use server_terminal::capabilities as terminal;
use server_voice::capabilities as voice;

use crate::Services;

/// Crate-owned group whose transport handler is selected by the API.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Group {
    Schedule(schedule::Group),
    Browser(browser::Group),
    Metadata(metadata::Group),
    Filesystem(filesystem::Group),
    Provider(provider::Group),
    Terminal(terminal::Group),
    Voice(voice::Group),
}

/// Merge crate-owned declarations without copying their method lists.
pub(super) fn implemented_groups() -> impl Iterator<Item = (Group, &'static [&'static str])> {
    schedule::IMPLEMENTED_GROUPS
        .iter()
        .map(|&(group, methods)| (Group::Schedule(group), methods))
        .chain(
            browser::IMPLEMENTED_GROUPS
                .iter()
                .map(|&(group, methods)| (Group::Browser(group), methods)),
        )
        .chain(
            voice::IMPLEMENTED_GROUPS
                .iter()
                .map(|&(group, methods)| (Group::Voice(group), methods)),
        )
        .chain(
            metadata::IMPLEMENTED_GROUPS
                .iter()
                .map(|&(group, methods)| (Group::Metadata(group), methods)),
        )
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
        push_tokens: services.push_tokens.is_some(),
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
            skills: services.skills.is_some(),
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
    .chain(voice::installed_capabilities(services.speech.is_some()))
    .chain(schedule::installed_capabilities(
        services.schedules.is_some(),
    ))
    .chain(browser::installed_capabilities(services.browser.is_some()))
    .map(str::to_owned)
    .collect()
}

#[cfg(test)]
mod tests;
