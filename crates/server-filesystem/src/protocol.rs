//! Filesystem protocol boundary.

pub mod checkout;
pub mod file_transfer;
pub mod files;
pub mod forge;
pub mod github_projects;
pub mod skills;
pub mod workspace_recovery;
pub mod worktrees;

pub(crate) fn valid_id(value: &str) -> bool {
    !value.is_empty() && value.len() <= 128 && !value.chars().any(char::is_control)
}
