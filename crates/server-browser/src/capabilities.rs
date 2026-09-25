//! Browser capability ownership.
/// Crate-owned routing group.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Group {
    /// Browser operations.
    Browser,
}
/// Implemented methods selected by the transport dispatcher.
pub const IMPLEMENTED_GROUPS: &[(Group, &[&str])] =
    &[(Group::Browser, crate::protocol::CAPABILITIES)];
/// Installed methods when the host composes this service.
pub fn installed_capabilities(installed: bool) -> impl Iterator<Item = &'static str> {
    crate::protocol::CAPABILITIES
        .iter()
        .copied()
        .filter(move |_| installed)
}
