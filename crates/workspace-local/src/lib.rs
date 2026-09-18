//! Native filesystem and Git adapter for local Project workspaces.

mod directories;
mod workspace;

pub use directories::DocumentsProjectDirectory;
pub use workspace::LocalProjectWorkspace;

#[cfg(test)]
mod directories_tests;
