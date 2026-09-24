//! Agent and Provider method groups and installation rules owned by this crate.

use crate::protocol;

/// Business method group selected by the transport after capability negotiation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Group {
    /// Versioned Agent presets.
    Agents,
    /// Agent runtime directory and lifecycle.
    AgentRuntime,
    /// Native Agent execution and turn configuration.
    AgentExecution,
}

/// Implemented method groups, including event methods; catalog placeholders are excluded.
pub const IMPLEMENTED_GROUPS: &[(Group, &[&str])] = &[
    (Group::Agents, protocol::agent::CAPABILITIES),
    (Group::AgentRuntime, protocol::agent_lifecycle::CAPABILITIES),
    (
        Group::AgentExecution,
        protocol::agent_execution::CAPABILITIES,
    ),
    (Group::AgentExecution, protocol::agent_config::CAPABILITIES),
];

/// Presence of independently composed services, supplied by the host.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct InstalledServices {
    /// Versioned Agent preset service.
    pub agents: bool,
    /// Agent runtime directory service.
    pub agent_runtime: bool,
    /// Native Agent execution service, which also implements runtime methods.
    pub agent_execution: bool,
}

/// Return methods supported by `services`, using this crate's installation rules.
///
/// The iterator borrows static method names and excludes uninstalled optional services.
pub fn installed_capabilities(services: InstalledServices) -> impl Iterator<Item = &'static str> {
    IMPLEMENTED_GROUPS
        .iter()
        .filter(move |(group, _)| match group {
            Group::Agents => services.agents,
            Group::AgentRuntime => services.agent_runtime || services.agent_execution,
            Group::AgentExecution => services.agent_execution,
        })
        .flat_map(|(_, methods)| methods.iter().copied())
}

#[cfg(test)]
mod tests;
