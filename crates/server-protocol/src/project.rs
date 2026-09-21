//! Public project DTOs; deliberately independent of domain/storage types.

use serde::{Deserialize, Serialize};

/// Project capabilities installed only when the project application service is present.
pub const CAPABILITIES: &[&str] = &[
    "project.open",
    "project.list",
    "project.get",
    "project.close",
];

/// Open an existing independent Git root; retries must retain the same key.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Open {
    /// Absolute local filesystem path; aliases are canonicalized before fingerprinting.
    pub path: String,
    /// Durable catalog-scoped ASCII key, 1–128 bytes.
    pub idempotency_key: String,
}

/// Read one catalog project, without implicitly acquiring its lease.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Get {
    /// Stable project UUID.
    pub project_id: String,
}

/// Release current project ownership after short transactions have drained.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Close {
    /// Stable project UUID.
    pub project_id: String,
    /// Current generation returned by get/list.
    pub owner_epoch: u64,
    /// Durable key scoped to the close method.
    pub idempotency_key: String,
}

/// Stable-ID keyset pagination of registered project summaries.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct List {
    /// Exclusive project UUID cursor; absent for the first page.
    #[serde(default)]
    pub after: Option<String>,
    /// Page size between 1 and 50, default 20.
    #[serde(default = "default_limit")]
    pub limit: usize,
}

fn default_limit() -> usize {
    20
}

/// Durable completion; lease state must be queried separately.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Receipt {
    /// Stable operation UUID, unchanged on retries/restarts.
    pub operation_id: String,
    /// Stable project UUID.
    pub project_id: String,
}

/// Rebuildable catalog facts plus this process's active lease generation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    /// Stable project UUID.
    pub project_id: String,
    /// Registered canonical directory.
    pub path: String,
    /// Display name frozen at initialization.
    pub name: String,
    /// Initial committed HEAD; reopening does not refresh it.
    pub base_commit: String,
    /// Initial immutable system Message UUID.
    pub root_message_id: String,
    /// Unix epoch milliseconds at initialization.
    pub created_at: u64,
    /// Current generation, or null when not open in this process.
    pub owner_epoch: Option<u64>,
}

/// Bounded catalog page, without instruction content or implicit project acquisition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Page {
    /// Summaries ordered by stable UUID.
    pub projects: Vec<Project>,
    /// Use as `after`; a full last page may be followed by an empty page.
    pub next_after: Option<String>,
}
