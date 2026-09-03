use std::path::PathBuf;

/// Stable identity of a registered project.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
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
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
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
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
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

/// A registered local project and its current instruction head.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Project {
    /// Project identity.
    pub id: ProjectId,
    /// Human-readable name.
    pub name: String,
    /// Canonical absolute Git root.
    pub workdir: PathBuf,
    /// Whether the manager initialized Git while registering the project.
    pub git_initialized_by_manager: bool,
    /// Current append-only instruction revision.
    pub instruction_revision: u64,
    /// Digest of the current structured instruction component.
    pub instruction_digest: String,
}

/// Audit summary for one instruction input. Exact content lives beside this
/// summary in the structured component snapshot.
#[derive(Clone, Debug, Eq, PartialEq)]
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
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InstructionSourceSnapshot {
    /// Source provenance and precedence.
    pub summary: InstructionSourceSummary,
    /// Exact UTF-8 source content captured for this revision.
    pub content: String,
}

/// Immutable, reproducible Project-instruction component.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InstructionSnapshot {
    /// Monotonic project-local revision, beginning at one.
    pub revision: u64,
    /// Source snapshots in strictly increasing priority order.
    pub sources: Vec<InstructionSourceSnapshot>,
    /// SHA-256 of the canonical component content and provenance.
    pub content_digest: String,
}

/// A typed component stored in an immutable System Message.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SystemMessageComponent {
    /// Project instruction sources captured at a particular revision.
    ProjectInstructions(InstructionSnapshot),
}

/// Immutable root system message for a new message tree.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SystemMessage {
    /// Message identity.
    pub id: MessageId,
    /// Owning project.
    pub project_id: ProjectId,
    /// Structured snapshots used later to assemble a provider prompt.
    pub components: Vec<SystemMessageComponent>,
}

/// A movable reference into a project's immutable message forest.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Session {
    /// Session identity.
    pub id: SessionId,
    /// Owning project.
    pub project_id: ProjectId,
    /// Current message pointer.
    pub current_message_id: MessageId,
    /// Compare-and-swap version.
    pub version: u64,
}

/// Atomic result of creating a new tree and a session pointing at its root.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionRoot {
    /// Newly created session.
    pub session: Session,
    /// Immutable root system message.
    pub root_message: SystemMessage,
}
