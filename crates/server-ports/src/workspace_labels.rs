//! Persistence boundary for atomic workspace label catalog and assignment changes.

use std::fmt::Debug;

use server_domain::registry::PersistedWorkspaceRecord;
use server_domain::workspace_labels::WorkspaceLabelDefinition;

/// Coherent catalog and workspace snapshot used to plan one label mutation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceLabelStoreSnapshot {
    /// Host-wide label definitions in insertion order.
    pub labels: Vec<WorkspaceLabelDefinition>,
    /// All workspace records, including archived workspaces.
    pub workspaces: Vec<PersistedWorkspaceRecord>,
}

/// Complete after-image for one compound catalog/assignment transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceLabelStoreMutation {
    /// Catalog observed while planning; prevents stale application commits.
    pub expected_labels: Vec<WorkspaceLabelDefinition>,
    /// Complete catalog after-image.
    pub labels: Vec<WorkspaceLabelDefinition>,
    /// Workspace records whose label assignment or timestamp changed.
    pub workspace_updates: Vec<PersistedWorkspaceRecord>,
}

/// Failure before commit, or an uncertain outcome that requires process restart.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum WorkspaceLabelStoreError {
    /// A catalog, transaction, or workspace document was invalid.
    #[error("invalid workspace label storage")]
    Invalid,
    /// The current catalog changed after the mutation was planned.
    #[error("workspace label catalog changed")]
    Conflict,
    /// A filesystem operation failed before a coherent commit was published.
    #[error("workspace label storage I/O failed")]
    Io,
    /// Commit and rollback could not establish one known durable outcome.
    #[error("workspace label storage outcome is uncertain")]
    Uncertain,
}

/// Blocking store; hosts serialize calls outside an async reactor.
pub trait WorkspaceLabelStore: Debug + Send + Sync {
    /// Load and recover any interrupted compound transaction.
    ///
    /// # Errors
    /// Returns invalid, I/O, or uncertain storage errors.
    fn initialize(&self) -> Result<(), WorkspaceLabelStoreError>;

    /// Return one coherent catalog/workspace snapshot.
    ///
    /// # Errors
    /// Returns invalid, I/O, or uncertain storage errors.
    fn snapshot(&self) -> Result<WorkspaceLabelStoreSnapshot, WorkspaceLabelStoreError>;

    /// Atomically publish a catalog and all assignment rewrites.
    ///
    /// # Errors
    /// Returns conflict, invalid, I/O, or uncertain storage errors.
    fn commit(
        &self,
        mutation: &WorkspaceLabelStoreMutation,
    ) -> Result<(), WorkspaceLabelStoreError>;
}
