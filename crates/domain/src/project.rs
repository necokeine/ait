use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::{AgentId, DomainError, DomainMetadata, ErrorCode, GitCommit, TimestampMs};

/// Stable identity of a registered project.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ProjectId(String);

impl ProjectId {
    /// Creates an externally assigned project identity.
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Returns the string representation.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Availability state of a registered Project.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectStatus {
    /// Workdir and Project database are usable.
    Active,
    /// Project is intentionally hidden from active use.
    Archived,
    /// Registered workdir no longer exists.
    Missing,
    /// Workdir exists but cannot currently be accessed.
    Unavailable,
}

/// A registered local Project and its current instruction head.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Project {
    /// Project identity.
    pub id: ProjectId,
    /// Human-readable name.
    pub name: String,
    /// Human-readable description; empty when none was supplied.
    #[serde(default)]
    pub description: String,
    /// Canonical absolute Git root.
    pub workdir: PathBuf,
    /// Whether the manager initialized Git while registering the project.
    pub git_initialized_by_manager: bool,
    /// Optional remote repository URL represented locally.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo_url: Option<String>,
    /// Repository HEAD captured once when the Project was registered.
    pub base_commit: GitCommit,
    /// Default Agent suggested when creating a new Session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_agent_id: Option<AgentId>,
    /// Current append-only instruction revision.
    pub instruction_revision: u64,
    /// Digest of the current structured instruction component.
    pub instruction_digest: String,
    /// Non-secret JSON-compatible Project extension data.
    #[serde(default)]
    pub metadata: DomainMetadata,
    /// Current availability state.
    pub status: ProjectStatus,
    /// Registration time.
    pub created_at: TimestampMs,
    /// Last mutable catalog update time.
    pub updated_at: TimestampMs,
}

impl Project {
    /// Validates Project identity, Git-root path shape, instruction head, and timestamps.
    ///
    /// Filesystem canonicalization and Git top-level equality require a Project
    /// environment port; this local check only rejects structurally invalid values.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorCode::InvalidProject`] for an invalid aggregate.
    pub fn validate(&self) -> Result<(), DomainError> {
        if self.id.as_str().is_empty()
            || self.name.trim().is_empty()
            || !self.workdir.is_absolute()
            || self
                .repo_url
                .as_ref()
                .is_some_and(|url| url.trim().is_empty())
            || !self.base_commit.is_valid()
            || self.instruction_revision == 0
            || !crate::common::is_sha256(&self.instruction_digest)
            || self.updated_at < self.created_at
        {
            return Err(DomainError::invariant(
                ErrorCode::InvalidProject,
                "project identity, workdir, instruction head, or timestamps are invalid",
            ));
        }
        Ok(())
    }
}

/// Revisioned default Agent suggestion for future Sessions.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ProjectDefaults {
    #[serde(default)]
    default_agent_id: Option<AgentId>,
    #[serde(default = "initial_revision")]
    revision: u64,
}

const fn initial_revision() -> u64 {
    1
}

impl Default for ProjectDefaults {
    fn default() -> Self {
        Self {
            default_agent_id: None,
            revision: 1,
        }
    }
}

impl ProjectDefaults {
    /// Returns the current suggestion; it does not rebind existing Sessions.
    #[must_use]
    pub fn agent(&self) -> Option<&AgentId> {
        self.default_agent_id.as_ref()
    }

    /// Returns the current catalog revision.
    #[must_use]
    pub const fn revision(&self) -> u64 {
        self.revision
    }

    /// Selects a validated named Agent in the surrounding catalog transaction.
    pub fn select(&mut self, agent: AgentId) {
        self.default_agent_id = Some(agent);
        self.mark_updated();
    }

    /// Clears the Project override so future Sessions use the global default.
    pub fn clear(&mut self) {
        self.default_agent_id = None;
        self.mark_updated();
    }

    /// Advances the catalog revision after Project metadata changes.
    pub fn mark_updated(&mut self) {
        self.revision = self.revision.saturating_add(1);
    }
}

/// Validates caller-owned registration metadata and normalizes the declared URL.
///
/// # Errors
///
/// Returns [`ErrorCode::InvalidProject`] for blank identities, names, or supplied URLs.
pub fn validate_registration(
    id: &str,
    name: &str,
    repo_url: &mut Option<String>,
) -> Result<(), DomainError> {
    if id.trim().is_empty() || name.trim().is_empty() {
        return Err(DomainError::invariant(
            ErrorCode::InvalidProject,
            "project id and name are required",
        ));
    }
    if let Some(url) = repo_url {
        *url = url.trim().to_owned();
        if url.is_empty() {
            return Err(DomainError::invariant(
                ErrorCode::InvalidProject,
                "repository URL cannot be empty",
            ));
        }
    }
    Ok(())
}

mod ownership;
#[cfg(test)]
mod tests;
pub use ownership::ProjectOwner;
