//! Metadata method groups and installation rules owned by this crate.

use crate::protocol;

/// Business method group selected by the transport after capability negotiation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Group {
    /// Connection metadata and shared subscription controls.
    Base,
    /// Connection events and activity heartbeats.
    Session,
    /// Project and Workspace records, configuration, and icons.
    Directory,
    /// Daemon configuration and lifecycle.
    Daemon,
    /// Workspace labels and their subscriptions.
    Labels,
    /// Workspace setup and scripts.
    Automation,
    /// Workspace attention state.
    WorkspaceState,
}

/// Implemented method groups, including event methods; catalog placeholders are excluded.
pub const IMPLEMENTED_GROUPS: &[(Group, &[&str])] = &[
    (Group::Base, protocol::server::CAPABILITIES),
    (Group::Session, &[protocol::server::HEARTBEAT_METHOD]),
    (Group::Session, protocol::session::CAPABILITIES),
    (Group::Directory, protocol::directory::CAPABILITIES),
    (Group::Directory, protocol::project_config::CAPABILITIES),
    (Group::Directory, protocol::project_icon::CAPABILITIES),
    (Group::Daemon, protocol::daemon::CAPABILITIES),
    (Group::Labels, protocol::workspace_labels::CAPABILITIES),
    (
        Group::Automation,
        protocol::workspace_automation::CAPABILITIES,
    ),
    (
        Group::WorkspaceState,
        protocol::workspace_state::CAPABILITIES,
    ),
];

/// Presence of independently composed services, supplied by the host.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
// Services are independently optional; every combination is meaningful.
#[allow(clippy::struct_excessive_bools)]
pub struct InstalledServices {
    /// Directory service.
    pub directory: bool,
    /// Daemon service.
    pub daemon: bool,
    /// Workspace label service.
    pub workspace_labels: bool,
    /// Workspace automation service.
    pub workspace_automation: bool,
    /// Workspace state service.
    pub workspace_state: bool,
}

/// Return methods supported by `services`, using this crate's installation rules.
///
/// The iterator borrows static method names and excludes uninstalled optional services.
pub fn installed_capabilities(services: InstalledServices) -> impl Iterator<Item = &'static str> {
    IMPLEMENTED_GROUPS
        .iter()
        .filter(move |(group, _)| match group {
            Group::Base | Group::Session => true,
            Group::Directory => services.directory,
            Group::Daemon => services.daemon,
            Group::Labels => services.workspace_labels,
            Group::Automation => services.workspace_automation,
            Group::WorkspaceState => services.workspace_state,
        })
        .flat_map(|(_, methods)| methods.iter().copied())
}

#[cfg(test)]
mod tests;
