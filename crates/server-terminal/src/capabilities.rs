//! Terminal method groups and installation rules owned by this crate.

use crate::protocol;

/// Business method group selected by the transport after capability negotiation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Group {
    /// PTY lifecycle, subscriptions, and input.
    Terminal,
}

/// Implemented method groups, including event methods; catalog placeholders are excluded.
pub const IMPLEMENTED_GROUPS: &[(Group, &[&str])] = &[(Group::Terminal, protocol::CAPABILITIES)];

/// Presence of independently composed services, supplied by the host.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct InstalledServices {
    /// Terminal service.
    pub terminals: bool,
}

/// Return methods supported by `services`, using this crate's installation rules.
///
/// The iterator borrows static method names and excludes uninstalled optional services.
pub fn installed_capabilities(services: InstalledServices) -> impl Iterator<Item = &'static str> {
    IMPLEMENTED_GROUPS
        .iter()
        .filter(move |(group, _)| match group {
            Group::Terminal => services.terminals,
        })
        .flat_map(|(_, methods)| methods.iter().copied())
}

#[cfg(test)]
mod tests;
