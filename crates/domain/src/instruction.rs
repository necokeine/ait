use serde::{Deserialize, Serialize};

use crate::{DomainError, ErrorCode};

/// Audit summary for one instruction input. Exact content lives beside this
/// summary in the structured component snapshot.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct InstructionSourceSummary {
    /// Stable, displayable source name.
    pub name: String,
    /// Locator relative to the project, or an explicitly authorized absolute locator.
    pub locator: String,
    /// Larger values override smaller values by being rendered later.
    pub priority: u32,
    /// SHA-256 of the source bytes.
    pub content_digest: String,
    /// Source size in bytes.
    pub byte_len: u64,
}

/// Immutable content and provenance of one discovered instruction source.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct InstructionSourceSnapshot {
    /// Source provenance and precedence.
    pub summary: InstructionSourceSummary,
    /// Exact UTF-8 source content captured for this revision.
    pub content: String,
}

/// Immutable, reproducible Project-instruction component.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct InstructionSnapshot {
    /// Monotonic project-local revision, beginning at one.
    pub revision: u64,
    /// Source snapshots in strictly increasing priority order.
    pub sources: Vec<InstructionSourceSnapshot>,
    /// SHA-256 of the canonical component content and provenance.
    pub content_digest: String,
}

impl InstructionSnapshot {
    /// Validates revision, digest formats, source sizes, and strict priority ordering.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorCode::InvalidProject`] when the snapshot cannot be
    /// reproduced deterministically.
    pub fn validate(&self) -> Result<(), DomainError> {
        let sources_valid = self.sources.iter().all(|source| {
            !source.summary.name.trim().is_empty()
                && !source.summary.locator.trim().is_empty()
                && crate::common::is_sha256(&source.summary.content_digest)
                && u64::try_from(source.content.len())
                    .is_ok_and(|length| length == source.summary.byte_len)
        });
        let priorities_strict = self
            .sources
            .windows(2)
            .all(|pair| pair[0].summary.priority < pair[1].summary.priority);
        if self.revision == 0
            || !crate::common::is_sha256(&self.content_digest)
            || !sources_valid
            || !priorities_strict
        {
            return Err(DomainError::invariant(
                ErrorCode::InvalidProject,
                "instruction revision, digest, source length, or priority order is invalid",
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
