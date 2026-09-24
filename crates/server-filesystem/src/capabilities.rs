//! Filesystem method groups and installation rules owned by this crate.

use crate::protocol;

/// Business method group selected by the transport after capability negotiation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Group {
    /// Git checkout operations and diff subscriptions.
    Checkout,
    /// Forge and pull request operations.
    Forge,
    /// File operations, subscriptions, and transfers.
    Files,
    /// GitHub repository discovery and provisioning.
    GithubProjects,
    /// Git worktree lifecycle.
    Worktrees,
    /// Archived Workspace recovery.
    WorkspaceRecovery,
}

/// Implemented method groups, including event methods; catalog placeholders are excluded.
pub const IMPLEMENTED_GROUPS: &[(Group, &[&str])] = &[
    (Group::Checkout, protocol::checkout::CAPABILITIES),
    (Group::Forge, protocol::forge::CAPABILITIES),
    (Group::Files, protocol::files::CAPABILITIES),
    (
        Group::GithubProjects,
        protocol::github_projects::CAPABILITIES,
    ),
    (Group::Worktrees, protocol::worktrees::CAPABILITIES),
    (
        Group::WorkspaceRecovery,
        protocol::workspace_recovery::CAPABILITIES,
    ),
];

/// Presence of independently composed services, supplied by the host.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
// Services are independently optional; every combination is meaningful.
#[allow(clippy::struct_excessive_bools)]
pub struct InstalledServices {
    /// Checkout service.
    pub checkout: bool,
    /// Forge service.
    pub forge: bool,
    /// File service.
    pub files: bool,
    /// GitHub projects service.
    pub github_projects: bool,
    /// Worktree service.
    pub worktrees: bool,
    /// Workspace recovery service.
    pub workspace_recovery: bool,
}

/// Return methods supported by `services`, using this crate's installation rules.
///
/// The iterator borrows static method names and excludes uninstalled optional services.
pub fn installed_capabilities(services: InstalledServices) -> impl Iterator<Item = &'static str> {
    IMPLEMENTED_GROUPS
        .iter()
        .filter(move |(group, _)| match group {
            Group::Checkout => services.checkout,
            Group::Forge => services.forge,
            Group::Files => services.files,
            Group::GithubProjects => services.github_projects,
            Group::Worktrees => services.worktrees,
            Group::WorkspaceRecovery => services.workspace_recovery,
        })
        .flat_map(|(_, methods)| methods.iter().copied())
}

#[cfg(test)]
mod tests;
