//! Installation and selection persistence boundary for orchestration skills.

use crate::protocol::skills::{Operation, Selection, Snapshot};
use server_model::ErrorCode;

/// Whether a frozen plan also commits a new selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApplyMode {
    /// Install/update only; preserve directories awaiting removal consent.
    Reconcile,
    /// Apply a confirmed selection transaction.
    Save,
    /// Explicitly remove every installed managed directory.
    Uninstall,
}

/// Serialized backend; external filesystem conflicts must fail without deleting foreign work.
pub trait SkillStore: Send + std::fmt::Debug {
    /// Recover an interrupted operation before exposing a snapshot.
    /// # Errors
    /// Returns safe storage/conflict errors; unresolved backups must be retained.
    fn recover(&mut self) -> Result<(), ErrorCode>;
    /// Read the explicitly persisted selection; absence means follow all bundled skills.
    /// # Errors
    /// Returns safe storage errors.
    fn selection(&self) -> Result<Option<Selection>, ErrorCode>;
    /// Persist an imported selection without modifying installed files.
    /// # Errors
    /// Returns safe storage errors.
    fn import(&mut self, selection: &Selection) -> Result<(), ErrorCode>;
    /// Inspect the current catalog and target contents for a desired selection.
    /// # Errors
    /// Rejects unsafe paths, excessive trees and unreadable storage.
    fn scan(&self, selection: &Selection) -> Result<Snapshot, ErrorCode>;
    /// Apply a fresh, bounded plan with rollback and restart recovery.
    /// # Errors
    /// Returns storage/conflict errors without discarding recovery records.
    fn apply(
        &mut self,
        selection: &Selection,
        ops: &[Operation],
        mode: ApplyMode,
    ) -> Result<(), ErrorCode>;
}
