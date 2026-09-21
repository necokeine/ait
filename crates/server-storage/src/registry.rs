//! File-backed Paseo Project/Workspace registries; hosts must hold a data-directory lease.
//! Rust translation and modifications: see third-party/paseo/NOTICE and LICENSE.

mod core;
mod listeners;
mod paths;
mod projects;
mod workspaces;

pub use projects::FileBackedProjectRegistry;
pub use workspaces::FileBackedWorkspaceRegistry;

use server_ports::registry::RegistryError;

/// Generate an opaque `wks_` identity with eight cryptographically random bytes.
///
/// # Errors
/// Returns I/O failure when the operating system cannot supply random bytes.
pub fn generate_workspace_id() -> Result<String, RegistryError> {
    generate_id("wks_")
}

fn generate_id(prefix: &str) -> Result<String, RegistryError> {
    let mut bytes = [0; 8];
    getrandom::fill(&mut bytes).map_err(|_| RegistryError::Io)?;
    Ok(format!("{prefix}{:016x}", u64::from_be_bytes(bytes)))
}

#[cfg(test)]
mod tests;
