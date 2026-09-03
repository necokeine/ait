use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::{AgentId, DomainError, DomainMetadata, ErrorCode, RunId, TimestampMs};

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

/// Stable identity of a session.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SessionId(String);

impl SessionId {
    /// Creates an externally assigned session identity.
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

/// Stable identity of an immutable message.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MessageId(String);

impl MessageId {
    /// Creates an externally assigned message identity.
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
    /// Optional human-readable description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Canonical absolute Git root.
    pub workdir: PathBuf,
    /// Whether the manager initialized Git while registering the project.
    pub git_initialized_by_manager: bool,
    /// Default Agent selected for new interactive Runs.
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
            || self.instruction_revision == 0
            || !is_sha256(&self.instruction_digest)
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
                && is_sha256(&source.summary.content_digest)
                && u64::try_from(source.content.len())
                    .is_ok_and(|length| length == source.summary.byte_len)
        });
        let priorities_strict = self
            .sources
            .windows(2)
            .all(|pair| pair[0].summary.priority < pair[1].summary.priority);
        if self.revision == 0
            || !is_sha256(&self.content_digest)
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

/// A typed component stored in an immutable System Message.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SystemMessageComponent {
    /// Project instruction sources captured at a particular revision.
    ProjectInstructions(InstructionSnapshot),
}

/// Immutable root system message for a new message tree.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SystemMessage {
    /// Message identity.
    pub id: MessageId,
    /// Owning project.
    pub project_id: ProjectId,
    /// Structured snapshots used later to assemble a provider prompt.
    pub components: Vec<SystemMessageComponent>,
}

/// Lifecycle of a named Session reference.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionStatus {
    /// May accept input and follow a Run.
    Active,
    /// Retained for history but unavailable for new work.
    Archived,
}

/// A movable reference into a Project's immutable Message forest.
///
/// A Session owns no Message history. `active_run_id` is only an exclusive
/// non-terminal Run binding, while `current_message_id` is moved by CAS.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Session {
    /// Session identity.
    pub id: SessionId,
    /// Owning project.
    pub project_id: ProjectId,
    /// Human-readable reference name.
    pub name: String,
    /// Optional UI title.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Current message pointer.
    pub current_message_id: MessageId,
    /// The sole non-terminal Run currently following this Session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_run_id: Option<RunId>,
    /// Agent selected when a caller does not provide one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_agent_id: Option<AgentId>,
    /// Session availability state.
    pub status: SessionStatus,
    /// Compare-and-swap version.
    pub version: u64,
    /// Creation time.
    pub created_at: TimestampMs,
    /// Last pointer, binding, or metadata update time.
    pub updated_at: TimestampMs,
}

impl Session {
    /// Creates an active, idle Session pointing at one existing Message.
    #[must_use]
    pub fn new(
        id: SessionId,
        project_id: ProjectId,
        name: impl Into<String>,
        current_message_id: MessageId,
        now: TimestampMs,
    ) -> Self {
        Self {
            id,
            project_id,
            name: name.into(),
            title: None,
            current_message_id,
            active_run_id: None,
            default_agent_id: None,
            status: SessionStatus::Active,
            version: 1,
            created_at: now,
            updated_at: now,
        }
    }

    /// Validates pointer, binding, version, and lifecycle fields.
    ///
    /// Cross-Project Message/Run checks require a store and remain application invariants.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorCode::InvalidSession`] for an invalid aggregate.
    pub fn validate(&self) -> Result<(), DomainError> {
        if self.id.as_str().is_empty()
            || self.project_id.as_str().is_empty()
            || self.name.trim().is_empty()
            || self.current_message_id.as_str().is_empty()
            || self.version == 0
            || (self.status == SessionStatus::Archived && self.active_run_id.is_some())
            || self.updated_at < self.created_at
        {
            return Err(DomainError::invariant(
                ErrorCode::InvalidSession,
                "session identity, pointer, binding, version, or timestamps are invalid",
            ));
        }
        Ok(())
    }
}

/// Atomic result of creating a new tree and a session pointing at its root.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SessionRoot {
    /// Newly created session.
    pub session: Session,
    /// Immutable root system message.
    pub root_message: SystemMessage,
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn instruction_source(priority: u32) -> InstructionSourceSnapshot {
        InstructionSourceSnapshot {
            summary: InstructionSourceSummary {
                name: format!("source-{priority}"),
                locator: format!("source-{priority}.md"),
                priority,
                content_digest: "a".repeat(64),
                byte_len: 4,
            },
            content: "test".into(),
        }
    }

    #[test]
    fn archived_session_cannot_retain_an_active_run() {
        let mut session = Session::new(
            SessionId::new("session-1"),
            ProjectId::new("project-1"),
            "main",
            MessageId::new("message-1"),
            TimestampMs(1),
        );
        session.status = SessionStatus::Archived;
        session.active_run_id = Some(RunId::new("run-1"));
        assert_eq!(
            session.validate().unwrap_err().code,
            ErrorCode::InvalidSession
        );
    }

    #[test]
    fn session_round_trip_keeps_snake_case_status() {
        let session = Session::new(
            SessionId::new("session-1"),
            ProjectId::new("project-1"),
            "main",
            MessageId::new("message-1"),
            TimestampMs(1),
        );
        let encoded = serde_json::to_string(&session).unwrap();
        assert!(encoded.contains("\"status\":\"active\""));
        assert_eq!(serde_json::from_str::<Session>(&encoded).unwrap(), session);
    }

    #[test]
    fn instruction_priorities_must_be_strictly_increasing() {
        let mut snapshot = InstructionSnapshot {
            revision: 1,
            sources: vec![instruction_source(10), instruction_source(20)],
            content_digest: "b".repeat(64),
        };
        snapshot.validate().unwrap();

        snapshot.sources.swap(0, 1);
        assert_eq!(
            snapshot.validate().unwrap_err().code,
            ErrorCode::InvalidProject
        );
    }
}
