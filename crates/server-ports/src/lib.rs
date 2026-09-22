//! Blocking adapter contracts. Callers must execute these ports outside async reactor threads.

pub mod agent;
pub mod agent_runtime;
pub mod checkout;
pub mod daemon;
pub mod forge;
pub mod provisioning;
pub mod registry;
pub mod workspace_automation;
pub mod workspace_labels;
pub mod workspace_recovery;
pub mod worktrees;

use server_domain::{GitCommit, MessageId, OperationId, OwnerEpoch, Project, ProjectId};
use std::fmt::Debug;
use std::path::{Path, PathBuf};

/// Safe, implementation-independent failures from project adapters and use cases.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ProjectError {
    /// Invalid parameters or persisted domain values.
    #[error("invalid project parameters or data")]
    Invalid,
    /// The directory is not a supported independent Git root with HEAD.
    #[error("an independent Git root with a valid HEAD is required")]
    UnsupportedWorkspace,
    /// A legacy managed directory was detected.
    #[error("legacy managed projects are not supported")]
    LegacyProject,
    /// A path or identity lease is held by another owner.
    #[error("project is owned by another process")]
    Busy,
    /// The database belongs to an unknown family or schema version.
    #[error("unsupported database format")]
    UnsupportedFormat,
    /// One key was reused with different normalized business parameters.
    #[error("idempotency key conflicts with a previous request")]
    IdempotencyConflict,
    /// A catalog path or identity conflicts with another registration.
    #[error("project identity conflicts with the catalog")]
    IdentityConflict,
    /// The requested project is absent from this catalog.
    #[error("project is not registered")]
    NotFound,
    /// This server has no active lease on the project.
    #[error("project is not open in this server")]
    NotOpen,
    /// The request carries an obsolete ownership generation.
    #[error("project owner has changed")]
    StaleOwner,
    /// Filesystem, Git, or storage operation could not complete.
    #[error("project I/O failed; retry with the same idempotency key")]
    Io,
}

impl From<server_domain::InvalidValue> for ProjectError {
    fn from(_: server_domain::InvalidValue) -> Self {
        Self::Invalid
    }
}

/// Result of read-only workspace inspection, prior to any initialization side effects.
#[derive(Debug, Clone)]
pub struct WorkspaceInfo {
    /// Canonical UTF-8 root, bounded to 4096 bytes without control characters.
    pub root: PathBuf,
    /// Current committed HEAD; no repository initialization is performed.
    pub head: GitCommit,
    /// Bounded root AGENTS.md snapshot, or empty when absent.
    pub instructions: String,
}

/// RAII ownership guard. Implementations must not unlink held lock files.
pub trait Lease: Debug + Send {}

/// Identity lease with a durable user-local generation high-water mark.
pub trait IdentityLease: Lease {
    /// Reserve a generation above both the local mark and `database_epoch`.
    /// The reservation must be durable before a database claim or response is published.
    ///
    /// # Errors
    /// Rejects corrupted/exhausted counters and I/O failures without reusing a generation.
    fn reserve_epoch(&mut self, database_epoch: OwnerEpoch) -> Result<OwnerEpoch, ProjectError>;
}

/// Local Git, path validation, and process-independent leases.
pub trait Workspace: Debug + Send {
    /// Inspect `path` without modifying user files. Errors identify unsupported or unreadable paths.
    ///
    /// # Errors
    /// Returns unsupported-path, legacy-project, invalid-content, or I/O errors.
    fn inspect(&self, path: &Path) -> Result<WorkspaceInfo, ProjectError>;
    /// Revalidate, acquire the canonical path lease, and ensure the local Git runtime exclusion.
    /// Errors preserve any already-created runtime directory; retries must be safe.
    ///
    /// # Errors
    /// Returns contention, changed-workspace, invalid-state, or I/O errors.
    fn acquire_path(&self, info: &WorkspaceInfo) -> Result<Box<dyn Lease>, ProjectError>;
    /// Acquire a user-local ID lease shared across every server data directory.
    /// Always acquire after the canonical path lease; contention returns `Busy`.
    ///
    /// # Errors
    /// Returns Busy for contention or an I/O/invalid-state error.
    fn acquire_identity(&self, id: ProjectId) -> Result<Box<dyn IdentityLease>, ProjectError>;
}

/// Authoritative project database, always held and dropped before its workspace leases.
pub trait ProjectStore: Debug + Send {
    /// Atomically initialize identity/root, or return existing facts unchanged.
    /// Caller must hold the path lease; an existing identity is read before taking the ID lease.
    ///
    /// # Errors
    /// Returns an invalid-domain, unsupported-format, or database error.
    fn initialize(&mut self, initial: &Project) -> Result<Project, ProjectError>;
    /// Read the database's last generation, with both leases held.
    ///
    /// # Errors
    /// Returns invalid-generation or storage errors.
    fn owner_epoch(&mut self) -> Result<OwnerEpoch, ProjectError>;
    /// Claim an already-reserved generation strictly above the database's last generation.
    ///
    /// # Errors
    /// Rejects obsolete generations or storage failures.
    fn claim(&mut self, reserved: OwnerEpoch) -> Result<(), ProjectError>;
    /// Reject writes/close requests carrying an obsolete generation.
    ///
    /// # Errors
    /// Returns `StaleOwner` for obsolete generations, or a storage error.
    fn check_owner(&mut self, expected: OwnerEpoch) -> Result<(), ProjectError>;
}

/// Opens only the independent project's schema, never the old Ait database.
pub trait ProjectStorage: Debug + Send {
    /// Open the database under `root/.ait-server`, with a path lease already held.
    ///
    /// # Errors
    /// Returns unsupported-format, unsafe-file, or storage errors.
    fn open(&self, root: &Path) -> Result<Box<dyn ProjectStore>, ProjectError>;
}

/// Rebuildable, bounded summary; message contents remain in the Project database.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogEntry {
    /// Stable project ID.
    pub id: ProjectId,
    /// Last registered canonical root.
    pub path: PathBuf,
    /// Initial display name.
    pub name: String,
    /// Initial Git baseline.
    pub base_commit: GitCommit,
    /// Initial system Message identity.
    pub root_message_id: MessageId,
    /// Creation time in Unix epoch milliseconds.
    pub created_at: u64,
}

impl CatalogEntry {
    /// Project creation facts projected into a local directory registration.
    #[must_use]
    pub fn from_project(project: &Project, path: PathBuf) -> Self {
        Self {
            id: project.id(),
            path,
            name: project.name().to_owned(),
            base_commit: project.base_commit().clone(),
            root_message_id: project.root().id(),
            created_at: project.root().created_at(),
        }
    }
}

/// Stable receipt; current lease state must be queried separately.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Receipt {
    /// Durable operation identity independent of the transport request ID.
    pub operation_id: OperationId,
    /// Project affected by this completed operation.
    pub project_id: ProjectId,
}

/// Durable open intent, committed before workspace or Project initialization.
#[derive(Debug, Clone)]
pub struct OpenIntent {
    /// Operation identity reused by retries.
    pub operation_id: OperationId,
    /// Canonical path used as the business fingerprint.
    pub path: PathBuf,
    /// Committed result, if the operation already completed.
    pub receipt: Option<Receipt>,
}

/// Catalog transactions for a single local authenticated principal.
/// Keys are scoped by this catalog and method; credentials/client IDs are not fingerprints.
pub trait Catalog: Debug + Send {
    /// Read or stage an open intent. A reused key with a different path is rejected.
    ///
    /// # Errors
    /// Returns `IdempotencyConflict` for changed parameters or a storage error.
    fn begin_open(&mut self, key: &str, path: &Path) -> Result<OpenIntent, ProjectError>;
    /// Atomically register the authoritative facts and complete the intent.
    ///
    /// # Errors
    /// Returns identity/intent conflicts or a storage error; the transaction rolls back.
    fn finish_open(
        &mut self,
        intent: &OpenIntent,
        entry: &CatalogEntry,
    ) -> Result<Receipt, ProjectError>;
    /// Read one registration without acquiring project ownership.
    ///
    /// # Errors
    /// Returns `NotFound` or a storage error.
    fn get(&mut self, id: ProjectId) -> Result<CatalogEntry, ProjectError>;
    /// Read at most `limit` summaries after a stable ID, without opening project databases.
    ///
    /// # Errors
    /// Returns invalid-limit or storage errors.
    fn list(
        &mut self,
        after: Option<ProjectId>,
        limit: usize,
    ) -> Result<Vec<CatalogEntry>, ProjectError>;
    /// Read a close receipt before evaluating a potentially obsolete owner generation.
    ///
    /// # Errors
    /// Returns `IdempotencyConflict` for changed parameters or a storage error.
    fn close_receipt(&mut self, key: &str, id: ProjectId) -> Result<Option<Receipt>, ProjectError>;
    /// Persist close completion while leases are still held, before releasing them.
    ///
    /// # Errors
    /// Returns conflicting-key or storage errors.
    fn finish_close(&mut self, key: &str, id: ProjectId) -> Result<Receipt, ProjectError>;
}
