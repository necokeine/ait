//! Blocking registry contracts corresponding to Paseo workspace-registry.ts.

use std::fmt::Debug;
use std::sync::Arc;

use server_domain::registry::{
    PersistedProjectKind, PersistedProjectRecord, PersistedWorkspaceRecord,
};

/// A registry operation failed before committing, or a project observer failed after commit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum RegistryError {
    /// Input does not satisfy the source record schema.
    #[error("invalid registry record")]
    InvalidRecord,
    /// Existing state could not be decoded; it was not replaced by an empty registry.
    #[error("invalid registry file")]
    InvalidFile,
    /// Reading or atomically replacing the file failed.
    #[error("registry I/O failed")]
    Io,
    /// Mutation was explicitly frozen until a new registry instance is created.
    #[error("registry mutations are blocked until restart")]
    Frozen,
    /// A project observer failed after the mutation was committed.
    #[error("project mutation committed but observer failed")]
    Observer,
}

/// Source lifecycle mutation kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MutationKind {
    /// A record was inserted or updated.
    Upsert,
    /// An existing record was archived.
    Archive,
    /// An existing record was removed.
    Remove,
}

/// A project event published after the durable write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectMutation {
    /// Mutation category.
    pub kind: MutationKind,
    /// Target identity, including when removed.
    pub project_id: String,
    /// Committed record, or none on removal.
    pub project: Option<PersistedProjectRecord>,
}

/// A workspace event published after the durable write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceMutation {
    /// Mutation category.
    pub kind: MutationKind,
    /// Target identity, including when removed.
    pub workspace_id: String,
    /// Committed record, or none on removal.
    pub workspace: Option<PersistedWorkspaceRecord>,
    /// Present only when explicitly true on upsert.
    pub expects_initial_agent: Option<bool>,
}

/// Context accompanying a workspace insertion.
#[derive(Debug, Clone, Copy, Default)]
pub struct WorkspaceMutationContext {
    /// A first agent is expected to follow this workspace creation.
    pub expects_initial_agent: Option<bool>,
}

/// Context accompanying a workspace archive.
#[derive(Debug, Clone, Default)]
pub struct WorkspaceArchiveContext {
    /// Merged change request whose automatic archive was consumed.
    pub auto_archived_change_request_url: Option<String>,
}

/// Input to source-compatible active-root project allocation.
#[derive(Debug, Clone)]
pub struct ActiveProjectInput {
    /// Lexical root path; symlinks are not resolved by the registry.
    pub root_path: String,
    /// Current root classification.
    pub kind: PersistedProjectKind,
    /// Derived name for a new project only.
    pub display_name: String,
    /// Current project grouping key.
    pub project_key: Option<String>,
    /// Timestamp string assigned by the caller.
    pub timestamp: String,
}

/// Post-commit observer. A workspace ignores observer errors; a project reports them.
pub type MutationListener<T> = Arc<dyn Fn(&T) -> Result<(), RegistryError> + Send + Sync>;

/// Dropping this registration unsubscribes the observer.
pub trait MutationSubscription: Debug + Send {}

/// Project registry. Calls are blocking and must run outside an async reactor.
/// Mutations on one instance are serialized; the host owns cross-process exclusion.
pub trait ProjectRegistry: Debug + Send + Sync {
    /// Load lazily. Errors preserve existing state and leave initialization retryable.
    ///
    /// # Errors
    /// Returns load errors or an unavailable/poisoned registry.
    fn initialize(&self) -> Result<(), RegistryError>;
    /// Check the registry file's existence without creating it.
    fn exists_on_disk(&self) -> bool;
    /// Return insertion-ordered records, including archived records.
    ///
    /// # Errors
    /// Returns load errors or an unavailable/poisoned registry.
    fn list(&self) -> Result<Vec<PersistedProjectRecord>, RegistryError>;
    /// Return a record or none for a missing identity.
    ///
    /// # Errors
    /// Returns load errors or an unavailable/poisoned registry.
    fn get(&self, id: &str) -> Result<Option<PersistedProjectRecord>, RegistryError>;
    /// Reuse the oldest active equivalent root, or allocate a new `prj_` identity.
    ///
    /// # Errors
    /// Returns storage/validation errors; observer errors occur after commit.
    fn get_or_create_active_by_root(
        &self,
        input: &ActiveProjectInput,
    ) -> Result<PersistedProjectRecord, RegistryError>;
    /// Validate and atomically insert/replace a record, then notify observers.
    ///
    /// # Errors
    /// Returns storage/validation errors; observer errors occur after commit.
    fn upsert(&self, record: &PersistedProjectRecord) -> Result<(), RegistryError>;
    /// Transform the latest record under serialization; missing identities return none.
    ///
    /// # Errors
    /// Returns storage/validation errors; observer errors occur after commit.
    fn update(
        &self,
        id: &str,
        update: &dyn Fn(&PersistedProjectRecord) -> PersistedProjectRecord,
    ) -> Result<Option<PersistedProjectRecord>, RegistryError>;
    /// Archive once; repeated archives and missing identities do not notify observers.
    ///
    /// # Errors
    /// Returns storage/validation errors; observer errors occur after commit.
    fn archive(&self, id: &str, timestamp: &str) -> Result<(), RegistryError>;
    /// Remove if present, notifying only after a committed change.
    ///
    /// # Errors
    /// Returns storage/validation errors; observer errors occur after commit.
    fn remove(&self, id: &str) -> Result<(), RegistryError>;
    /// Subscribe until the returned handle is dropped.
    fn subscribe_to_mutations(
        &self,
        listener: MutationListener<ProjectMutation>,
    ) -> Box<dyn MutationSubscription>;
}

/// Workspace registry with independent identities, including multiple records at one cwd.
pub trait WorkspaceRegistry: Debug + Send + Sync {
    /// Load lazily; a malformed file is an error and remains untouched.
    ///
    /// # Errors
    /// Returns load errors or an unavailable/poisoned registry.
    fn initialize(&self) -> Result<(), RegistryError>;
    /// Check the registry file's existence without creating it.
    fn exists_on_disk(&self) -> bool;
    /// Return insertion-ordered records, including archived records.
    ///
    /// # Errors
    /// Returns load errors or an unavailable/poisoned registry.
    fn list(&self) -> Result<Vec<PersistedWorkspaceRecord>, RegistryError>;
    /// Return a record or none for a missing identity.
    ///
    /// # Errors
    /// Returns load errors or an unavailable/poisoned registry.
    fn get(&self, id: &str) -> Result<Option<PersistedWorkspaceRecord>, RegistryError>;
    /// Validate and atomically insert/replace a record, then notify observers.
    ///
    /// # Errors
    /// Returns validation, load, write or frozen-registry errors.
    fn upsert(
        &self,
        record: &PersistedWorkspaceRecord,
        context: WorkspaceMutationContext,
    ) -> Result<(), RegistryError>;
    /// Transform the latest record under serialization; missing identities return none.
    ///
    /// # Errors
    /// Returns validation, load, write or frozen-registry errors.
    fn update(
        &self,
        id: &str,
        update: &dyn Fn(&PersistedWorkspaceRecord) -> PersistedWorkspaceRecord,
    ) -> Result<Option<PersistedWorkspaceRecord>, RegistryError>;
    /// Refresh archive timestamps, preserving existing auto-archive metadata unless replaced.
    ///
    /// # Errors
    /// Returns validation, load, write or frozen-registry errors.
    fn archive(
        &self,
        id: &str,
        timestamp: &str,
        context: &WorkspaceArchiveContext,
    ) -> Result<(), RegistryError>;
    /// Remove if present, notifying only after a committed change.
    ///
    /// # Errors
    /// Returns validation, load, write or frozen-registry errors.
    fn remove(&self, id: &str) -> Result<(), RegistryError>;
    /// Subscribe until the returned handle is dropped; observer errors cannot undo a commit.
    fn subscribe_to_mutations(
        &self,
        listener: MutationListener<WorkspaceMutation>,
    ) -> Box<dyn MutationSubscription>;
    /// Reject further mutations until this instance is replaced; reads remain available.
    ///
    /// # Errors
    /// Returns a load error or an unavailable/poisoned registry.
    fn block_all_mutations_until_restart(&self) -> Result<(), RegistryError>;
}
