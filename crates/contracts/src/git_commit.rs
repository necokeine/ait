//! User-visible outcome of optional Ait Git finalization.
use serde::{Deserialize, Serialize};

/// Frozen auto-commit state, independent of Codex's execution result.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunCommitStatus {
    /// Enabled and waiting for a successful native result.
    Pending,
    /// Exact commit object durably recorded before ref publication.
    Prepared,
    /// Published and acknowledged.
    Committed,
    /// No changes or unsafe baseline; no automatic commit was made.
    Skipped,
    /// Git-only finalization can be retried without executing Codex again.
    Failed,
}

/// Git outcome attached to a Run, never to an immutable Message.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RunGitCommit {
    /// Current Git finalization status.
    pub status: RunCommitStatus,
    /// Exact prepared or published object identity.
    pub commit_id: Option<String>,
    /// Short reason for a skip or failure.
    pub reason: Option<String>,
}
