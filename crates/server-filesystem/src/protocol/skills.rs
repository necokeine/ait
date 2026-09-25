//! Skill selection and installation method declarations; implementation is still pending.

/// Canonical get status request, currently a placeholder.
pub const GET_STATUS: &str = "agent.skills.get_status.request";
/// Canonical reconcile request, currently a placeholder.
pub const RECONCILE: &str = "agent.skills.reconcile.request";
/// Canonical uninstall request, currently a placeholder.
pub const UNINSTALL: &str = "agent.skills.uninstall.request";
/// Canonical save selection request, currently a placeholder.
pub const SAVE_SELECTION: &str = "agent.skills.save_selection.request";
/// Canonical import legacy selection request, currently a placeholder.
pub const IMPORT_LEGACY_SELECTION: &str = "agent.skills.import_legacy_selection.request";

/// Declared skill requests; these are not implemented capabilities.
pub const METHODS: &[&str] = &[
    GET_STATUS,
    RECONCILE,
    UNINSTALL,
    SAVE_SELECTION,
    IMPORT_LEGACY_SELECTION,
];
