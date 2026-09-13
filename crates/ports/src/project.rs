use std::path::{Path, PathBuf};

use ait_domain::{DomainError, GitCommit};

/// Allocates a new workdir for a Project whose caller supplied only a name.
/// Platform default-directory resolution and filename rules belong to the adapter.
#[async_trait::async_trait]
pub trait ProjectDirectoryCreator: Send + Sync {
    /// Creates exactly one new directory and returns its absolute path.
    ///
    /// Existing entries (including files and symlinks) must fail without mutation.
    /// No parent directories may be implicitly created. Once returned, the path
    /// is retained even if Git preparation or registration subsequently fails.
    /// Async admission and filesystem phases consume one deadline. A timed-out
    /// creation returns `ProjectDirectoryCreationFailed` with `reason = "timeout"`.
    /// If mkdir completed across that deadline, the error includes its retained
    /// path/state in both details and message, and is not automatically retryable.
    ///
    /// # Errors
    ///
    /// Returns a stable Project failure for invalid names, unavailable default
    /// directories, existing targets, or failed directory creation.
    async fn create_workdir(&self, name: &str) -> Result<PathBuf, DomainError>;
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

    /// Returns the repository HEAD, or `None` for an unborn repository.
    ///
    /// # Errors
    ///
    /// Returns an [`EnvironmentError`] when Git cannot inspect HEAD or returns
    /// an invalid full object identity.
    fn git_head(&self, directory: &Path) -> Result<Option<GitCommit>, EnvironmentError>;

    /// Creates the manager-owned empty initial commit for an unborn repository.
    ///
    /// # Errors
    ///
    /// Returns an [`EnvironmentError`] when Git cannot create the commit.
    fn git_commit_initial(&self, directory: &Path) -> Result<(), EnvironmentError>;

    /// Reports whether the index and worktree, including untracked files, are clean.
    ///
    /// # Errors
    ///
    /// Returns an [`EnvironmentError`] when Git cannot inspect repository status.
    fn git_is_clean(&self, directory: &Path) -> Result<bool, EnvironmentError>;

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
