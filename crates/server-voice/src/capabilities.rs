//! Voice capability ownership and installation.

/// Speech capability group selected by API routing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Group {
    /// Voice conversations and dictation streams.
    Voice,
}

/// All implemented speech methods, including client events.
pub const IMPLEMENTED_GROUPS: &[(Group, &[&str])] =
    &[(Group::Voice, crate::protocol::CAPABILITIES)];

/// Return installed methods when a speech service is composed by the host.
pub fn installed_capabilities(installed: bool) -> impl Iterator<Item = &'static str> {
    crate::protocol::CAPABILITIES
        .iter()
        .copied()
        .filter(move |_| installed)
}
