//! Version and ownership evidence for catalog and Project transactions.
use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// One Project snapshot, tied to the runtime that currently owns its storage.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ProjectVersion {
    /// Runtime instance holding the Project's operating-system locks.
    pub runtime_instance_id: String,
    /// Durable acquisition generation.
    pub owner_epoch: u64,
    /// Project-local business revision.
    pub revision: u64,
}

impl ProjectVersion {
    /// Execution identity without the business revision, which changes during a Run.
    #[must_use]
    pub fn owner(&self, project_id: &str) -> ait_domain::ProjectOwner {
        ait_domain::ProjectOwner {
            project_id: project_id.into(),
            runtime_instance_id: self.runtime_instance_id.clone(),
            owner_epoch: self.owner_epoch,
        }
    }
}

/// Read dependencies validated before a control-plane commit.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct ControlVersion {
    /// Identity of the configuration catalog; empty for legacy embedded stores.
    pub catalog_id: String,
    /// Revision of the configuration catalog.
    pub catalog_revision: u64,
    /// True when global configuration was read, rather than just a routing hint.
    pub observes_catalog: bool,
    /// Revisions and ownership evidence of Projects actually read.
    pub projects: BTreeMap<String, ProjectVersion>,
}

impl ControlVersion {
    /// Constructs a version for the legacy single-revision storage contract.
    #[must_use]
    pub fn legacy(revision: u64) -> Self {
        Self {
            catalog_revision: revision,
            observes_catalog: true,
            ..Self::default()
        }
    }

    /// Whether overlapping dependencies agree across two stages of a read plan.
    #[must_use]
    pub fn compatible_with(&self, other: &Self) -> bool {
        self.catalog_id == other.catalog_id
            && (!(self.observes_catalog || other.observes_catalog)
                || self.catalog_revision == other.catalog_revision)
            && self
                .projects
                .iter()
                .all(|(id, version)| other.projects.get(id).is_none_or(|other| other == version))
    }

    /// Checks acquisition identities across retries without comparing business revisions.
    #[must_use]
    pub fn same_owners(&self, other: &Self) -> bool {
        self.catalog_id == other.catalog_id
            && self.projects.iter().all(|(id, version)| {
                other
                    .projects
                    .get(id)
                    .is_some_and(|other| version.owner(id) == other.owner(id))
            })
    }
}

#[cfg(test)]
mod tests;
