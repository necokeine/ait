use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::{
    AgentId, DomainError, ErrorCode, MessageId, ProjectId, RunId, SystemMessage, TimestampMs,
};

/// Stable identity of a Session.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SessionId(String);

impl SessionId {
    /// Creates an externally assigned Session identity.
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
    /// Owning Project.
    pub project_id: ProjectId,
    /// Manager-owned linked worktree used for every interactive execution.
    pub workdir: PathBuf,
    /// Human-readable reference name.
    #[serde(default)]
    pub name: String,
    /// Optional UI title.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// AI-generated plain-text summary used by Session search.
    #[serde(default)]
    pub description: String,
    /// Current Message pointer.
    pub current_message_id: MessageId,
    /// The sole non-terminal Run currently following this Session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_run_id: Option<RunId>,
    /// Agent used by the next interactive Run; mutable only while idle.
    pub agent_id: AgentId,
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
        workdir: PathBuf,
        name: impl Into<String>,
        current_message_id: MessageId,
        agent_id: AgentId,
        now: TimestampMs,
    ) -> Self {
        Self {
            id,
            project_id,
            workdir,
            name: name.into(),
            title: None,
            description: String::new(),
            current_message_id,
            active_run_id: None,
            agent_id,
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
            || !self.workdir.is_absolute()
            || self.current_message_id.as_uuid().is_nil()
            || self.agent_id.as_str().is_empty()
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

/// Atomic result of creating a new Message tree and a Session pointing at its root.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SessionRoot {
    /// Newly created Session.
    pub session: Session,
    /// Immutable root System Message.
    pub root_message: SystemMessage,
}

/// The mutable reference owned by a Session; it owns no Message history.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SessionReference {
    current_message_id: MessageId,
    active_run_id: Option<RunId>,
    agent_id: AgentId,
    version: u64,
}

impl SessionReference {
    /// Creates an idle reference at version one.
    #[must_use]
    pub const fn new(head: MessageId, agent_id: AgentId) -> Self {
        Self {
            current_message_id: head,
            active_run_id: None,
            agent_id,
            version: 1,
        }
    }

    /// Returns the current immutable Message identity.
    #[must_use]
    pub const fn head(&self) -> MessageId {
        self.current_message_id
    }

    /// Returns the current Agent binding.
    #[must_use]
    pub fn agent(&self) -> &AgentId {
        &self.agent_id
    }

    /// Returns the Run that exclusively owns this reference.
    #[must_use]
    pub fn active_run(&self) -> Option<&RunId> {
        self.active_run_id.as_ref()
    }

    /// Returns the version compared in the surrounding atomic transaction.
    #[must_use]
    pub const fn version(&self) -> u64 {
        self.version
    }

    /// Advances only a matching pointer/version to a direct child.
    ///
    /// # Errors
    ///
    /// Returns a pointer conflict without modifying the reference on stale input.
    pub fn advance(
        &mut self,
        expected: MessageId,
        version: u64,
        parent: Option<MessageId>,
        child: MessageId,
    ) -> Result<(), DomainError> {
        if self.current_message_id != expected
            || self.version != version
            || parent != Some(expected)
            || child == expected
        {
            return Err(DomainError::invariant(
                ErrorCode::SessionPointerConflict,
                "Session pointer changed or output is not a direct child",
            ));
        }
        self.current_message_id = child;
        self.version = self.version.saturating_add(1);
        Ok(())
    }

    /// Claims an idle Session for its next Run.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorCode::SessionBusy`] if a Run already owns the reference.
    pub fn acquire(&mut self, run: RunId) -> Result<(), DomainError> {
        if self.active_run_id.is_some() {
            return Err(DomainError::invariant(
                ErrorCode::SessionBusy,
                "session already has an active run",
            ));
        }
        self.active_run_id = Some(run);
        Ok(())
    }

    /// Changes the Agent only while idle, incrementing the version only on change.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorCode::SessionBusy`] when a Run owns the reference.
    pub fn bind(&mut self, agent: AgentId) -> Result<(), DomainError> {
        if self.active_run_id.is_some() {
            return Err(DomainError::invariant(
                ErrorCode::SessionBusy,
                "session already has an active run",
            ));
        }
        if self.agent_id != agent {
            self.agent_id = agent;
            self.version = self.version.saturating_add(1);
        }
        Ok(())
    }

    /// Records an Agent configuration change, including a new revision of the same Agent.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorCode::SessionBusy`] while a Run owns the reference.
    pub fn configure(&mut self, agent: AgentId) -> Result<(), DomainError> {
        let version = self.version;
        self.bind(agent)?;
        self.version = version.saturating_add(1);
        Ok(())
    }

    /// Releases only the matching Run; a late worker cannot release a newer owner.
    pub fn release(&mut self, run: &RunId) {
        if self.active_run_id.as_ref() == Some(run) {
            self.active_run_id = None;
            self.version = self.version.saturating_add(1);
        }
    }
}

#[cfg(test)]
mod tests;
