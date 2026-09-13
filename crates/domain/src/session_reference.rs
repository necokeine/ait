//! Pure Session pointer, binding and version transitions shared by application use cases.
use crate::{AgentId, DomainError, ErrorCode, MessageId, RunId};
use serde::{Deserialize, Serialize};

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
    /// Current immutable Message identity.
    #[must_use]
    pub const fn head(&self) -> MessageId {
        self.current_message_id
    }
    /// Current Agent binding.
    #[must_use]
    pub fn agent(&self) -> &AgentId {
        &self.agent_id
    }
    /// Run that exclusively owns this reference.
    #[must_use]
    pub fn active_run(&self) -> Option<&RunId> {
        self.active_run_id.as_ref()
    }
    /// Version compared in the surrounding atomic transaction.
    #[must_use]
    pub const fn version(&self) -> u64 {
        self.version
    }
    /// Advances only a matching pointer/version to a direct child.
    /// # Errors
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
    /// # Errors
    /// Returns `SessionBusy` if a Run already owns the reference.
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
    /// # Errors
    /// Returns `SessionBusy` when a Run owns the reference.
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
    /// # Errors
    /// Returns `SessionBusy` while a Run owns the reference.
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
mod tests {
    use super::*;

    #[test]
    fn stale_pointer_or_version_never_moves_the_reference() {
        let root = MessageId::from_u128(1);
        let child = MessageId::from_u128(2);
        let mut reference = SessionReference::new(root, AgentId::new("agent"));
        let original = reference.clone();
        assert!(reference.advance(root, 0, Some(root), child).is_err());
        assert_eq!(reference, original);
        assert!(reference.advance(root, 1, None, child).is_err());
        assert_eq!(reference, original);
        reference.advance(root, 1, Some(root), child).unwrap();
        assert_eq!(reference.head(), child);
        assert_eq!(reference.version(), 2);
    }

    #[test]
    fn busy_binding_and_late_release_preserve_the_new_owner() {
        let mut reference = SessionReference::new(MessageId::from_u128(1), AgentId::new("agent"));
        reference.acquire(RunId::new("old")).unwrap();
        assert_eq!(
            reference.bind(AgentId::new("other")).unwrap_err().code,
            ErrorCode::SessionBusy
        );
        reference.release(&RunId::new("old"));
        reference.acquire(RunId::new("new")).unwrap();
        let expected = reference.clone();
        reference.release(&RunId::new("old"));
        assert_eq!(reference, expected);
        let encoded = serde_json::to_value(&reference).unwrap();
        assert_eq!(
            serde_json::from_value::<SessionReference>(encoded).unwrap(),
            reference
        );
    }
}
