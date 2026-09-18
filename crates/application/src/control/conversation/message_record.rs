//! Persisted Message conversion at the conversation/domain boundary.
use std::collections::HashMap;

use ait_domain::{
    DomainError, DomainMetadata, ErrorCode, GitCommit, Message, MessageId, MessageKind,
    MessageOrigin, MessageRole, ProjectId, SubMessage, TimestampMs,
};

use super::record::MessageRecord;

fn invalid() -> DomainError {
    DomainError::invariant(ErrorCode::InvalidMessageId, "invalid persisted Message")
}

impl TryFrom<&MessageRecord> for Message {
    type Error = DomainError;

    fn try_from(state: &MessageRecord) -> Result<Self, Self::Error> {
        if let Some(native) = state
            .data
            .as_ref()
            .and_then(|data| data.get("native_message"))
        {
            let message: Message = serde_json::from_value(native.clone()).map_err(|_| invalid())?;
            if message.id.to_string() != state.id
                || message.project_id.as_str() != state.project_id
                || message.parent_message_id.map(|id| id.to_string()) != state.parent_message_id
                || message.role != state.role
                || message.kind != state.kind
                || message.created_at.0 != state.created_at
                || message
                    .git_commit
                    .as_ref()
                    .map(ait_domain::GitCommit::as_str)
                    != state.git_commit.as_deref()
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
            if state.text.as_deref() != (!text.is_empty()).then_some(text.as_str()) {
                return Err(invalid());
            }
            message.validate().map_err(|_| invalid())?;
            return Ok(message);
        }
        let message = Message {
            id: MessageId::parse(&state.id).map_err(|_| invalid())?,
            project_id: ProjectId::new(&state.project_id),
            parent_message_id: state
                .parent_message_id
                .as_deref()
                .map(MessageId::parse)
                .transpose()
                .map_err(|_| invalid())?,
            role: state.role,
            // Portable tool-result placeholders contain no executable tool identity.
            kind: MessageKind::Standard,
            origin: if state.role == MessageRole::System {
                MessageOrigin::Project
            } else if state.git_commit.is_some() {
                MessageOrigin::Human
            } else {
                MessageOrigin::Agent
            },
            sub_messages: vec![SubMessage::Text {
                text: state.text.clone().unwrap_or_default(),
            }],
            created_by_session_id: None,
            run_id: None,
            run_seq: None,
            tool_result: None,
            git_commit: state
                .git_commit
                .as_ref()
                .map(GitCommit::parse)
                .transpose()?,
            metadata: DomainMetadata::default(),
            created_at: TimestampMs(state.created_at),
        };
        message.validate().map_err(|_| invalid())?;
        Ok(message)
    }
}

impl TryFrom<MessageRecord> for Message {
    type Error = DomainError;

    fn try_from(state: MessageRecord) -> Result<Self, Self::Error> {
        Self::try_from(&state)
    }
}

impl From<Message> for MessageRecord {
    fn from(message: Message) -> Self {
        let text = message
            .sub_messages
            .iter()
            .filter_map(|part| match part {
                SubMessage::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<String>();
        Self {
            id: message.id.to_string(),
            project_id: message.project_id.as_str().to_owned(),
            parent_message_id: message.parent_message_id.map(|id| id.to_string()),
            role: message.role,
            kind: message.kind,
            text: (!text.is_empty()).then_some(text),
            created_at: message.created_at.0,
            git_commit: message
                .git_commit
                .as_ref()
                .map(|commit| commit.as_str().to_owned()),
            data: Some(serde_json::json!({"native_message": message})),
        }
    }
}

impl From<&Message> for MessageRecord {
    fn from(message: &Message) -> Self {
        message.clone().into()
    }
}

pub(in crate::control) fn domain_path(
    messages: &[MessageRecord],
    head: &str,
) -> Result<Vec<Message>, DomainError> {
    let entities = messages
        .iter()
        .map(Message::try_from)
        .map(|result| result.map(|entity| (entity.id, entity)))
        .collect::<Result<HashMap<_, _>, _>>()?;
    ait_domain::message::message_path(MessageId::parse(head).map_err(|_| invalid())?, &entities)
        .map(|path| path.into_iter().cloned().collect())
}

#[cfg(test)]
mod tests;
