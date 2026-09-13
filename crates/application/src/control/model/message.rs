//! Legacy record and native Message conversion at the domain boundary.
use super::MessageState;
use ait_domain::{
    DomainError, DomainMetadata, ErrorCode, GitCommit, Message, MessageId, MessageKind,
    MessageOrigin, MessageRole, ProjectId, SubMessage, TimestampMs,
};
use std::collections::HashMap;
fn invalid() -> DomainError {
    DomainError::invariant(ErrorCode::InvalidMessageId, "invalid persisted Message")
}
impl MessageState {
    pub(in crate::control) fn entity(&self) -> Result<Message, DomainError> {
        if let Some(native) = self.data.as_ref().and_then(|d| d.get("native_message")) {
            let message: Message = serde_json::from_value(native.clone()).map_err(|_| invalid())?;
            if message.id.to_string() != self.id
                || message.project_id.as_str() != self.project_id
                || message.parent_message_id.map(|id| id.to_string()) != self.parent_message_id
                || message.role != self.role
                || message.kind != self.kind
                || message.created_at.0 != self.created_at
                || message
                    .git_commit
                    .as_ref()
                    .map(ait_domain::GitCommit::as_str)
                    != self.git_commit.as_deref()
            {
                return Err(invalid());
            }
            let text = message
                .sub_messages
                .iter()
                .filter_map(|part| match part {
                    SubMessage::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<String>();
            if self.text.as_deref() != (!text.is_empty()).then_some(text.as_str()) {
                return Err(invalid());
            }
            message.validate().map_err(|_| invalid())?;
            return Ok(message);
        }
        let message = Message {
            id: MessageId::parse(&self.id).map_err(|_| invalid())?,
            project_id: ProjectId::new(&self.project_id),
            parent_message_id: self
                .parent_message_id
                .as_deref()
                .map(MessageId::parse)
                .transpose()
                .map_err(|_| invalid())?,
            role: self.role,
            // Portable tool-result placeholders contain no executable tool identity.
            kind: MessageKind::Standard,
            origin: if self.role == MessageRole::System {
                MessageOrigin::Project
            } else if self.git_commit.is_some() {
                MessageOrigin::Human
            } else {
                MessageOrigin::Agent
            },
            sub_messages: vec![SubMessage::Text {
                text: self.text.clone().unwrap_or_default(),
            }],
            created_by_session_id: None,
            run_id: None,
            run_seq: None,
            tool_result: None,
            git_commit: self.git_commit.as_ref().map(GitCommit::parse).transpose()?,
            metadata: DomainMetadata::default(),
            created_at: TimestampMs(self.created_at),
        };
        message.validate().map_err(|_| invalid())?;
        Ok(message)
    }
}
pub(in crate::control) fn domain_path(
    messages: &[MessageState],
    head: &str,
) -> Result<Vec<Message>, DomainError> {
    let entities = messages
        .iter()
        .map(|m| m.entity().map(|entity| (entity.id, entity)))
        .collect::<Result<HashMap<_, _>, _>>()?;
    ait_domain::message_path::message_path(MessageId::parse(head).map_err(|_| invalid())?, |id| {
        entities.get(id)
    })
    .map(|path| path.into_iter().cloned().collect())
}
