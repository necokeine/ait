//! GitHub search and clone with shared Project registration.
use server_metadata::model::registry::PersistedProjectRecord;
use server_metadata::service::directory::{Directory, parse_remote};

pub use crate::ports::github_projects::{
    GithubCloneProtocol, GithubProjectsError, GithubProjectsRuntime, GithubRepository,
    GithubRepositoryVisibility,
};
/// GitHub clone outcome, including a checkout left behind when registration fails.
#[derive(Debug, Clone, PartialEq)]
pub struct GithubCloneOutcome {
    /// Normalized owner/repository path, or the original input when validation fails.
    pub repo: String,
    /// Completed checkout path; present even if Project registration then fails.
    pub checkout_path: Option<String>,
    /// Registered Project after a successful clone.
    pub project: Option<PersistedProjectRecord>,
    /// Safe business failure; no transport error is needed for an expected clone failure.
    pub error: Option<String>,
}

/// Coordinates GitHub provisioning and metadata registration.
#[derive(Debug)]
pub struct GithubProjects {
    directory: Directory,
    github: Box<dyn GithubProjectsRuntime>,
}

impl GithubProjects {
    /// Compose a runtime and a Directory sharing the host's metadata adapters.
    #[must_use]
    pub fn new(directory: Directory, github: Box<dyn GithubProjectsRuntime>) -> Self {
        Self { directory, github }
    }
    /// Search GitHub repositories using the host CLI and its configured clone protocol.
    ///
    /// # Errors
    /// Returns CLI availability, authentication, command, or response failures.
    pub fn search_github_repositories(
        &self,
        query: &str,
        limit: usize,
    ) -> Result<Vec<GithubRepository>, GithubProjectsError> {
        self.github.search_repositories(query, limit)
    }

    /// Clone a GitHub repository, then register its completed checkout as a Project.
    ///
    /// The checkout remains on disk if Project registration fails, matching Paseo's observable
    /// response with a non-null `checkoutPath` and null `project`.
    #[must_use]
    pub fn clone_github_project(
        &self,
        repo: &str,
        protocol: Option<GithubCloneProtocol>,
        target_directory: &str,
        timestamp: &str,
    ) -> GithubCloneOutcome {
        let original_repo = repo.trim().to_owned();
        let Some((name, display_name, clone_url)) = normalize_clone_repository(repo, protocol)
        else {
            return GithubCloneOutcome {
                repo: original_repo,
                checkout_path: None,
                project: None,
                error: Some("Repository must use owner/repo format or a GitHub remote URL with a valid clone protocol".to_owned()),
            };
        };
        let checkout_path = self.github.checkout_path(target_directory, &name).ok();
        let checkout_path = match self
            .github
            .clone_repository(&clone_url, target_directory, &name)
        {
            Ok(path) => path,
            Err(error) => {
                return GithubCloneOutcome {
                    repo: display_name,
                    checkout_path,
                    project: None,
                    error: Some(error.to_string()),
                };
            }
        };
        match self.directory.add_project(&checkout_path, timestamp) {
            Ok(project) => GithubCloneOutcome {
                repo: display_name,
                checkout_path: Some(checkout_path),
                project: Some(project),
                error: None,
            },
            Err(error) => GithubCloneOutcome {
                repo: display_name,
                checkout_path: Some(checkout_path),
                project: None,
                error: Some(error.to_string()),
            },
        }
    }
}
fn normalize_clone_repository(
    repo: &str,
    protocol: Option<GithubCloneProtocol>,
) -> Option<(String, String, String)> {
    let trimmed = repo.trim();
    if trimmed.len() < 3 || trimmed.len() > 4096 || trimmed.chars().any(char::is_control) {
        return None;
    }
    let direct_url = trimmed.starts_with("https://github.com/")
        || trimmed.starts_with("git@github.com:")
        || trimmed.starts_with("ssh://git@github.com/");
    if direct_url {
        if trimmed.contains(['?', '#']) {
            return None;
        }
        let remote = parse_remote(trimmed)?;
        if remote.host != "github.com" || remote.port.is_some() {
            return None;
        }
        let (owner, name) = clone_owner_and_name(&remote.path)?;
        return Some((
            name.to_owned(),
            format!("{owner}/{name}"),
            trimmed.to_owned(),
        ));
    }
    if trimmed.contains("://") || trimmed.contains('@') || trimmed.contains(':') {
        return None;
    }
    let (owner, name) = clone_owner_and_name(trimmed)?;
    let protocol = protocol?;
    let clone_url = match protocol {
        GithubCloneProtocol::Https => format!("https://github.com/{owner}/{name}.git"),
        GithubCloneProtocol::Ssh => format!("git@github.com:{owner}/{name}.git"),
    };
    Some((name.to_owned(), format!("{owner}/{name}"), clone_url))
}

fn clone_owner_and_name(path: &str) -> Option<(&str, &str)> {
    let (owner, raw_name) = path.split_once('/')?;
    let name = raw_name.strip_suffix(".git").unwrap_or(raw_name);
    (valid_clone_segment(owner) && valid_clone_segment(name)).then_some((owner, name))
}

fn valid_clone_segment(value: &str) -> bool {
    !value.is_empty()
        && value != "."
        && value != ".."
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

#[cfg(test)]
mod tests;
