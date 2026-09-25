//! Filesystem service boundary.

pub mod checkout;
pub mod files;
pub mod forge;
pub mod github_projects;
pub mod transfer;
pub mod uploads;
pub mod workspace_recovery;
pub mod worktrees;

/// Orchestration skill installation and selection.
pub mod skills;
