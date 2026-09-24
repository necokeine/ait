//! Atomic catalog operations for versioned Agent presets and explicit default selection.

use std::fmt::Debug;

use server_domain::agent::{AgentConfig, AgentSnapshot, AgentTarget, Revision};
use server_domain::{AgentId, OperationId};

/// Safe Agent failures, with no database diagnostics or credential input.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum AgentError {
    /// Invalid configuration, key, version, or page size.
    #[error("invalid Agent parameters or data")]
    Invalid,
    /// No preset has this identity.
    #[error("Agent not found")]
    NotFound,
    /// No immutable revision has this number.
    #[error("Agent revision not found")]
    RevisionNotFound,
    /// A conditional edit observed an obsolete head.
    #[error("Agent revision has changed")]
    RevisionConflict,
    /// Default selection changed after the caller read it.
    #[error("default Agent selection has changed")]
    DefaultConflict,
    /// A disabled preset cannot be selected as default.
    #[error("Agent is disabled")]
    Disabled,
    /// Clear or replace the default before disabling its preset.
    #[error("default Agent must be deselected before disabling it")]
    IsDefault,
    /// The same method/key was already used for different parameters.
    #[error("idempotency key conflicts with a previous request")]
    IdempotencyConflict,
    /// A transient catalog lock is held elsewhere.
    #[error("catalog is busy")]
    Busy,
    /// The database has an unsupported schema or corrupt facts.
    #[error("unsupported catalog format")]
    UnsupportedFormat,
    /// Storage could not complete the operation.
    #[error("Agent I/O failed; retry with the same key")]
    Io,
}

impl From<server_domain::InvalidValue> for AgentError {
    fn from(_: server_domain::InvalidValue) -> Self {
        Self::Invalid
    }
}

/// Full replacement command, committed with an immutable revision and receipt.
#[derive(Debug, Clone)]
pub struct ConfigureAgent {
    /// Creation or conditional edit.
    pub target: AgentTarget,
    /// Validated non-secret fields.
    pub config: AgentConfig,
    /// Method-scoped durable retry key.
    pub key: String,
    /// Server time; excluded from the business fingerprint.
    pub recorded_at: u64,
}

/// Stable completion, which may identify a historical revision after later edits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AgentReceipt {
    /// Durable operation UUID.
    pub operation_id: OperationId,
    /// Stable preset identity.
    pub agent_id: AgentId,
    /// Revision published by this operation.
    pub revision: Revision,
}

/// Explicit global default, initially empty at version zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DefaultSelection {
    /// Selected preset; no implicit first-Agent fallback.
    pub agent_id: Option<AgentId>,
    /// Monotonic compare-and-swap version within SQLite's integer range.
    pub version: u64,
}

/// Conditional replacement of the global default.
#[derive(Debug, Clone)]
pub struct SelectDefault {
    /// Desired preset or an explicit clear.
    pub agent_id: Option<AgentId>,
    /// Observed selection version.
    pub expected_version: u64,
    /// Durable method-scoped retry key.
    pub key: String,
}

/// Stable default-change completion; query current selection separately.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DefaultReceipt {
    /// Durable operation UUID.
    pub operation_id: OperationId,
    /// Selection as committed by this operation.
    pub selection: DefaultSelection,
}

/// Blocking catalog port. Every write checks receipts before current-state preconditions.
pub trait AgentCatalog: Debug + Send {
    /// Atomically append a revision, advance the head, and persist a receipt.
    ///
    /// # Errors
    /// Rejects conflicting retries, obsolete revisions, disabling a default, or storage failures.
    fn configure(&mut self, command: &ConfigureAgent) -> Result<AgentReceipt, AgentError>;
    /// Read an exact immutable revision, or the current head when absent.
    ///
    /// # Errors
    /// Returns missing Agent/revision or storage errors.
    fn get_agent(
        &mut self,
        id: AgentId,
        revision: Option<Revision>,
    ) -> Result<AgentSnapshot, AgentError>;
    /// Read current heads by stable ID, at most 50, after an exclusive cursor.
    ///
    /// # Errors
    /// Rejects page sizes outside 1–50 or storage errors.
    fn list_agents(
        &mut self,
        after: Option<AgentId>,
        limit: usize,
    ) -> Result<Vec<AgentSnapshot>, AgentError>;
    /// Read the current explicit default without testing execution or resolving credentials.
    ///
    /// # Errors
    /// Returns storage or invalid-state errors.
    fn get_default(&mut self) -> Result<DefaultSelection, AgentError>;
    /// Atomically change the default and record its receipt, even for a same-value new operation.
    ///
    /// # Errors
    /// Rejects conflicting retries, stale versions, missing/disabled presets, or storage failures.
    fn set_default(&mut self, command: &SelectDefault) -> Result<DefaultReceipt, AgentError>;
}
