//! Host GitHub repository discovery and clone boundary for project provisioning.

use std::fmt::Debug;

/// GitHub clone transport selected by the caller or host configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GithubCloneProtocol {
    /// Clone over HTTPS.
    Https,
    /// Clone over SSH.
    Ssh,
}

/// Normalized repository row returned by GitHub CLI discovery.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GithubRepository {
    /// GitHub GraphQL identity or CLI numeric identity as text.
    pub id: String,
    /// Repository name without owner.
    pub name: String,
    /// Full owner/repository path.
    pub name_with_owner: String,
    /// Optional description.
    pub description: Option<String>,
    /// Public or private visibility.
    pub visibility: GithubRepositoryVisibility,
    /// GitHub update timestamp.
    pub updated_at: String,
    /// Clone URL honoring the host Git protocol setting.
    pub clone_url: String,
}

/// Visibility reported by the available GitHub CLI repository commands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GithubRepositoryVisibility {
    /// Public repository.
    Public,
    /// Private repository.
    Private,
}

/// Safe failure category for GitHub project operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum GithubProjectsError {
    /// `gh` is absent from the server PATH.
    #[error("GitHub CLI (gh) is not installed or not in PATH")]
    CliMissing,
    /// `gh` needs a host login.
    #[error("GitHub CLI is not authenticated. Run gh auth login on the host.")]
    Unauthenticated,
    /// Repository discovery returned invalid data or otherwise failed.
    #[error("GitHub repository search failed")]
    SearchFailed,
    /// The requested clone destination is unsafe or unavailable.
    #[error("Invalid clone target directory")]
    InvalidTarget,
    /// The destination already exists.
    #[error("Checkout path already exists")]
    TargetExists,
    /// Git clone failed without publishing a checkout.
    #[error("GitHub repository clone failed")]
    CloneFailed,
}

/// Blocking local CLI and filesystem operations; callers run this outside the async reactor.
pub trait GithubProjectsRuntime: Debug + Send + Sync {
    /// List recent owned repositories for an empty query, otherwise search accessible repos.
    ///
    /// # Errors
    /// Returns a categorized CLI, authentication, or parsing failure.
    fn search_repositories(
        &self,
        query: &str,
        limit: usize,
    ) -> Result<Vec<GithubRepository>, GithubProjectsError>;

    /// Resolve the checkout path before cloning, so failures can report the intended path.
    ///
    /// # Errors
    /// Returns an invalid-target error when the parent path or child name cannot be resolved.
    fn checkout_path(
        &self,
        target_directory: &str,
        name: &str,
    ) -> Result<String, GithubProjectsError>;

    /// Clone one validated GitHub URL into a new child under `target_directory`.
    ///
    /// The adapter creates missing parent directories, stages the clone, and atomically renames
    /// the completed checkout. It checks for an existing target before the rename; simultaneous
    /// cross-process creation still needs a future no-replace primitive.
    ///
    /// # Errors
    /// Returns a categorized path, collision, or clone failure.
    fn clone_repository(
        &self,
        clone_url: &str,
        target_directory: &str,
        name: &str,
    ) -> Result<String, GithubProjectsError>;
}
