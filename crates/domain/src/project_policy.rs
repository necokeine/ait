//! Pure Project catalog policies; Git and canonical path facts remain adapter inputs.
use crate::{AgentId, DomainError, ErrorCode};
use serde::{Deserialize, Serialize};

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
    /// Current suggestion; it does not rebind existing Sessions.
    #[must_use]
    pub fn agent(&self) -> Option<&AgentId> {
        self.default_agent_id.as_ref()
    }
    /// Current catalog revision.
    #[must_use]
    pub const fn revision(&self) -> u64 {
        self.revision
    }
    /// Selects a validated named Agent in the surrounding catalog transaction.
    pub fn select(&mut self, agent: AgentId) {
        self.default_agent_id = Some(agent);
        self.revision = self.revision.saturating_add(1);
    }
}

/// Validates caller-owned registration metadata and normalizes the declared URL.
/// # Errors
/// Rejects blank identities, names and supplied repository URLs.
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
