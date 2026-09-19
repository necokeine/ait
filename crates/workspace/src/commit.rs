//! Git-only Run finalization. These values never contain model messages.
use serde::{Deserialize, Serialize};

/// Clean, frozen repository identity observed before model execution.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RunCommitBaseline {
    /// Original HEAD object.
    pub head: String,
    /// Original symbolic branch, absent for a detached HEAD.
    pub branch: Option<String>,
    /// Original index tree.
    pub index_tree: String,
}

/// Durable receipt prepared before changing the branch or index.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RunCommitPlan {
    /// Original repository baseline.
    pub baseline: RunCommitBaseline,
    /// Exact generated tree.
    pub tree: String,
    /// Commit object created without moving HEAD.
    pub commit_id: String,
}
