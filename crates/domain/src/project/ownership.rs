//! Pure identities used to fence runtime execution; no host locking behavior.
use serde::{Deserialize, Serialize};

/// One acquisition of a Project by one backend process.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ProjectOwner {
    /// Stable Project identity.
    pub project_id: String,
    /// Unique backend startup identity.
    pub runtime_instance_id: String,
    /// Monotonic Project acquisition generation.
    pub owner_epoch: u64,
}
