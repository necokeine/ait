//! Paseo-shaped GitHub repository search and project clone payloads.

use serde::{Deserialize, Serialize};

use crate::workspace::WorkspaceProjectDescriptorPayload;

/// GitHub project methods backed by the host CLI and project registry.
pub const CAPABILITIES: &[&str] = &[
    "workspace.github.search_repositories.request",
    "project.github.clone.request",
];

/// Repository discovery input. Empty query lists recent owned repositories.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct GithubRepositorySearchRequest {
    /// Search text, trimmed before invoking GitHub CLI.
    pub query: String,
    /// Maximum result count, 1–50; defaults to 20.
    pub limit: Option<usize>,
}

/// GitHub repository visibility supported by the CLI's `isPrivate` field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GithubRepositoryVisibility {
    /// Public repository.
    Public,
    /// Private repository.
    Private,
}

/// Normalized GitHub repository in a search result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GithubRepositoryPayload {
    /// GraphQL or numeric GitHub identity as text.
    pub id: String,
    /// Repository name.
    pub name: String,
    /// Full owner/repository path.
    pub name_with_owner: String,
    /// Optional description.
    pub description: Option<String>,
    /// Public or private visibility.
    pub visibility: GithubRepositoryVisibility,
    /// GitHub update timestamp.
    pub updated_at: String,
    /// Clone URL chosen from host GitHub CLI configuration.
    pub clone_url: String,
}

/// GitHub repository search status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GithubRepositorySearchStatus {
    /// Search completed.
    Success,
    /// GitHub CLI is absent.
    Unavailable,
    /// GitHub CLI needs authentication.
    Unauthenticated,
    /// Another search or parsing failure occurred.
    Error,
}

/// Search result with Paseo's availability and optional missing-CLI reason.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GithubRepositorySearchResult {
    /// Search status.
    pub status: GithubRepositorySearchStatus,
    /// Matching repositories; empty for failure states.
    pub repositories: Vec<GithubRepositoryPayload>,
    /// Whether the CLI was available for this request.
    pub available: bool,
    /// `gh_missing` only for the unavailable status.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<&'static str>,
    /// Null on success, otherwise a safe diagnostic.
    pub error: Option<String>,
}

/// Clone transport for an owner/repository input.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GithubCloneProtocol {
    /// HTTPS remote.
    Https,
    /// SSH remote.
    Ssh,
}

/// Clone a GitHub repository into a new child of the target directory.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectGithubCloneRequest {
    /// Owner/repository pair or supported GitHub clone URL.
    pub repo: String,
    /// Optional transport for an owner/repository pair.
    pub clone_protocol: Option<GithubCloneProtocol>,
    /// Parent directory; `~` and relative paths follow host resolution.
    pub target_directory: String,
}

/// Completed clone and Project registration, or an inline business error.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectGithubCloneResult {
    /// Normalized owner/repository path when valid.
    pub repo: String,
    /// Completed checkout path, even if later registration fails.
    pub checkout_path: Option<String>,
    /// Registered Project, if any.
    pub project: Option<WorkspaceProjectDescriptorPayload>,
    /// Safe business failure or null on success.
    pub error: Option<String>,
}

#[cfg(test)]
mod tests;
