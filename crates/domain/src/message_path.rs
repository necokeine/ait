//! Pure immutable Message path validation, independent from any storage adapter.
use crate::{DomainError, ErrorCode, Message, MessageId, MessageRole};
use std::collections::HashSet;

/// Resolves one root-to-head path, checking cycles, ownership and the root role.
/// # Errors
/// Returns a stable domain error for a missing node, cycle, cross-Project parent or invalid root.
pub fn message_path<'a>(
    head: MessageId,
    lookup: impl Fn(&MessageId) -> Option<&'a Message>,
) -> Result<Vec<&'a Message>, DomainError> {
    let mut path = Vec::new();
    let mut seen = HashSet::new();
    let mut cursor = Some(head);
    let mut project = None;
    while let Some(id) = cursor {
        if !seen.insert(id) {
            return Err(DomainError::invariant(
                ErrorCode::InvalidMessageId,
                "message path contains a cycle",
            ));
        }
        let message = lookup(&id).ok_or_else(|| {
            DomainError::invariant(ErrorCode::MessageNotFound, "message path is incomplete")
        })?;
        if project.is_some_and(|p| p != &message.project_id) {
            return Err(DomainError::invariant(
                ErrorCode::SessionMessageProjectMismatch,
                "message path belongs to another project",
            ));
        }
        project = Some(&message.project_id);
        cursor = message.parent_message_id;
        path.push(message);
    }
    path.reverse();
    if path.first().is_none_or(|m| m.role != MessageRole::System) {
        return Err(DomainError::invariant(
            ErrorCode::InvalidMessageRole,
            "message path must begin with a system root",
        ));
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DomainMetadata, MessageKind, MessageOrigin, ProjectId, TimestampMs};
    use std::collections::HashMap;

    fn node(id: u128, parent: Option<u128>, role: MessageRole) -> Message {
        Message {
            id: MessageId::from_u128(id),
            project_id: ProjectId::new("p"),
            parent_message_id: parent.map(MessageId::from_u128),
            role,
            kind: MessageKind::Standard,
            origin: MessageOrigin::Agent,
            sub_messages: Vec::new(),
            created_by_session_id: None,
            run_id: None,
            run_seq: None,
            tool_result: None,
            git_commit: None,
            metadata: DomainMetadata::default(),
            created_at: TimestampMs(0),
        }
    }

    #[test]
    fn path_checks_missing_cycles_ownership_and_root_without_mutating_history() {
        let root = node(1, None, MessageRole::System);
        let child = node(2, Some(1), MessageRole::User);
        let mut messages = HashMap::from([(root.id, root), (child.id, child)]);
        let head = MessageId::from_u128(2);
        let ids: Vec<_> = message_path(head, |id| messages.get(id))
            .unwrap()
            .iter()
            .map(|m| m.id)
            .collect();
        assert_eq!(ids, vec![MessageId::from_u128(1), head]);
        assert_eq!(
            message_path(MessageId::from_u128(3), |id| messages.get(id))
                .unwrap_err()
                .code,
            ErrorCode::MessageNotFound
        );
        messages.get_mut(&head).unwrap().project_id = ProjectId::new("another");
        assert_eq!(
            message_path(head, |id| messages.get(id)).unwrap_err().code,
            ErrorCode::SessionMessageProjectMismatch
        );
        messages.get_mut(&head).unwrap().project_id = ProjectId::new("p");
        messages
            .get_mut(&MessageId::from_u128(1))
            .unwrap()
            .parent_message_id = Some(head);
        assert_eq!(
            message_path(head, |id| messages.get(id)).unwrap_err().code,
            ErrorCode::InvalidMessageId
        );
        let root = messages.get_mut(&MessageId::from_u128(1)).unwrap();
        root.parent_message_id = None;
        root.role = MessageRole::User;
        let original = messages.clone();
        assert_eq!(
            message_path(head, |id| messages.get(id)).unwrap_err().code,
            ErrorCode::InvalidMessageRole
        );
        assert_eq!(messages, original);
    }
}
