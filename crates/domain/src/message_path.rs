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
mod tests;
