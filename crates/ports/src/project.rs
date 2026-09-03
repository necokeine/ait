use std::path::{Path, PathBuf};

use ait_domain::{InstructionSourceSummary, MessageId, Project, ProjectId, SessionId, SessionRoot};

/// Fully rendered instruction material before a store assigns its revision.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiscoveredInstructions {
    /// Ordered source provenance.
    pub sources: Vec<InstructionSourceSummary>,
    /// Exact system prompt.
    pub rendered_prompt: String,
    /// SHA-256 of the exact system prompt.
    pub content_digest: String,
}

/// Input for the atomic new-tree/session write.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CreateSessionRoot {
    /// Project that owns the tree.
    pub project_id: ProjectId,
    /// Externally assigned session identity.
    pub session_id: SessionId,
    /// Externally assigned root message identity.
    pub root_message_id: MessageId,
    /// Current instruction material discovered immediately before the write.
    pub instructions: DiscoveredInstructions,
}

/// Stable failures exposed by a local project environment adapter.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EnvironmentError {
    /// The path does not exist.
    NotFound(PathBuf),
    /// The path is not a directory.
    NotDirectory(PathBuf),
    /// A project-relative path was absolute or contained traversal.
    InvalidRelativePath(PathBuf),
    /// Canonical resolution escaped the project root or explicit authorization root.
    OutOfScope(PathBuf),
    /// Git could not inspect or initialize the directory.
    Git(String),
    /// Another operating-system failure occurred.
    Io(String),
}

impl std::fmt::Display for EnvironmentError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound(path) => write!(formatter, "path not found: {}", path.display()),
            Self::NotDirectory(path) => {
                write!(formatter, "path is not a directory: {}", path.display())
            }
            Self::InvalidRelativePath(path) => {
                write!(
                    formatter,
                    "invalid project-relative path: {}",
                    path.display()
                )
            }
            Self::OutOfScope(path) => {
                write!(
                    formatter,
                    "path is outside the authorized scope: {}",
                    path.display()
                )
            }
            Self::Git(message) => write!(formatter, "git operation failed: {message}"),
            Self::Io(message) => write!(formatter, "filesystem operation failed: {message}"),
        }
    }
}

impl std::error::Error for EnvironmentError {}

/// Filesystem and Git capabilities required by project use cases.
pub trait ProjectEnvironment: Send + Sync {
    /// Validates a directory and returns its canonical absolute path.
    ///
    /// # Errors
    ///
    /// Returns an [`EnvironmentError`] when the path is missing, is not a
    /// directory, or cannot be canonicalized.
    fn canonicalize_directory(&self, path: &Path) -> Result<PathBuf, EnvironmentError>;

    /// Returns the canonical Git top-level, or `None` when the directory is not in a repository.
    ///
    /// # Errors
    ///
    /// Returns an [`EnvironmentError`] when Git cannot be executed or its
    /// successful output cannot be canonicalized.
    fn git_top_level(&self, directory: &Path) -> Result<Option<PathBuf>, EnvironmentError>;

    /// Initializes a Git repository in `directory`.
    ///
    /// # Errors
    ///
    /// Returns an [`EnvironmentError`] when Git cannot initialize the directory.
    fn git_init(&self, directory: &Path) -> Result<(), EnvironmentError>;

    /// Reads an optional project-relative instruction file without allowing symlink escape.
    ///
    /// # Errors
    ///
    /// Returns an [`EnvironmentError`] for invalid relative paths, scope
    /// escapes, unreadable paths, and non-regular files.
    fn read_project_file(
        &self,
        project_root: &Path,
        relative_path: &Path,
    ) -> Result<Option<Vec<u8>>, EnvironmentError>;

    /// Reads an optional absolute file under an explicitly authorized external root.
    ///
    /// # Errors
    ///
    /// Returns an [`EnvironmentError`] when the authorization root is invalid,
    /// the file is outside it, or the file cannot be read safely.
    fn read_authorized_file(
        &self,
        authorized_root: &Path,
        absolute_path: &Path,
    ) -> Result<Option<Vec<u8>>, EnvironmentError>;
}

/// Stable persistence failures for project orchestration.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StoreError {
    /// A canonical workdir is already registered.
    ProjectPathAlreadyRegistered(PathBuf),
    /// The project does not exist.
    ProjectNotFound(ProjectId),
    /// A supplied message does not belong to the project.
    MessageProjectMismatch,
    /// A generated identity is already in use.
    IdentityConflict(String),
    /// Adapter-specific failure.
    Other(String),
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ProjectPathAlreadyRegistered(path) => {
                write!(
                    formatter,
                    "project path is already registered: {}",
                    path.display()
                )
            }
            Self::ProjectNotFound(id) => write!(formatter, "project not found: {}", id.as_str()),
            Self::MessageProjectMismatch => write!(formatter, "message belongs to another project"),
            Self::IdentityConflict(id) => write!(formatter, "identity already exists: {id}"),
            Self::Other(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for StoreError {}

/// Persistence transaction boundary for Project registration and Session roots.
pub trait ProjectStore: Send + Sync {
    /// Inserts a project and its first instruction revision atomically.
    ///
    /// Implementations must enforce canonical workdir uniqueness.
    ///
    /// # Errors
    ///
    /// Returns a [`StoreError`] when the Project identity or canonical path is
    /// already registered, or when the atomic write fails.
    fn register_project(
        &self,
        project: Project,
        initial_instructions: DiscoveredInstructions,
    ) -> Result<Project, StoreError>;

    /// Returns a registered project.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::ProjectNotFound`] when the identity is unknown, or
    /// another [`StoreError`] when the store cannot be read.
    fn get_project(&self, project_id: &ProjectId) -> Result<Project, StoreError>;

    /// Atomically appends an instruction revision when the digest changed, creates
    /// an immutable root System message containing the selected snapshot, and
    /// creates a Session pointing at that root.
    ///
    /// # Errors
    ///
    /// Returns a [`StoreError`] when the Project is missing, an assigned
    /// identity conflicts, or the transaction cannot be committed.
    fn create_session_root(&self, command: CreateSessionRoot) -> Result<SessionRoot, StoreError>;
}
