use serde::{Deserialize, Serialize};

use crate::{DomainError, DomainMetadata, ErrorCode, MessageId, ProjectId, SessionId, TimestampMs};

/// Stable identity of a Run that produced a Message.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RunId(String);

impl RunId {
    /// Creates an externally assigned Run identity.
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

/// Role of an immutable Message.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageRole {
    /// Human, scheduler, system-injected, or `ToolResult` input.
    User,
    /// Root instruction snapshot.
    System,
    /// Agent output.
    Assistant,
}

/// Protocol kind of a Message.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageKind {
    /// Ordinary user, system, or assistant content.
    Standard,
    /// A special user Message answering a prior `ToolUse`.
    ToolResult,
}

/// Actor or subsystem that created a Message.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageOrigin {
    /// Project instruction discovery.
    Project,
    /// Interactive human input.
    Human,
    /// Agent output.
    Agent,
    /// Tool execution output.
    Tool,
    /// Scheduled input.
    Scheduler,
    /// Host-generated input.
    System,
}

/// Final status represented by a `ToolResult` Message.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolResultStatus {
    /// Tool execution succeeded.
    Succeeded,
    /// Tool execution failed.
    Failed,
    /// Approval was denied.
    Denied,
    /// Tool execution was cancelled.
    Cancelled,
}

/// ToolResult-specific fields carried by a user Message.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ToolResult {
    /// Provider-stable `ToolUse` call identity.
    pub call_id: String,
    /// Final execution status.
    pub status: ToolResultStatus,
    /// Bounded structured result, when available.
    pub output: Option<String>,
    /// Bounded error summary, when available.
    pub error: Option<String>,
}

/// A tool request embedded in an assistant Message.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ToolUse {
    /// Provider-stable call identity, unique within its Run.
    pub call_id: String,
    /// Registered tool name.
    pub tool_name: String,
    /// Canonical structured arguments.
    pub arguments: String,
    /// Optional provider-specific, non-secret metadata.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_metadata: Option<String>,
}

/// One ordered part inside a Message.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SubMessage {
    /// Plain text.
    Text {
        /// Text content.
        text: String,
    },
    /// Reference to an attachment stored outside the Message body.
    FileRef {
        /// Attachment identity.
        attachment_id: String,
        /// MIME media type.
        media_type: String,
        /// Optional display name.
        name: Option<String>,
    },
    /// Tool request emitted inside an assistant Message.
    ToolUse(ToolUse),
    /// Typed structured content encoded in a canonical representation.
    StructuredData {
        /// Content media type.
        media_type: String,
        /// Canonical encoded value.
        value: String,
    },
}

impl Serialize for SubMessage {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        #[derive(Serialize)]
        #[serde(tag = "type", rename_all = "snake_case")]
        enum Wire<'a> {
            Text {
                text: &'a str,
            },
            FileRef {
                attachment_id: &'a str,
                media_type: &'a str,
                name: &'a Option<String>,
            },
            ToolUse {
                call_id: &'a str,
                tool_name: &'a str,
                arguments: &'a str,
                provider_metadata: &'a Option<String>,
            },
            StructuredData {
                media_type: &'a str,
                value: &'a str,
            },
        }

        match self {
            Self::Text { text } => Wire::Text { text }.serialize(serializer),
            Self::FileRef {
                attachment_id,
                media_type,
                name,
            } => Wire::FileRef {
                attachment_id,
                media_type,
                name,
            }
            .serialize(serializer),
            Self::ToolUse(tool_use) => Wire::ToolUse {
                call_id: &tool_use.call_id,
                tool_name: &tool_use.tool_name,
                arguments: &tool_use.arguments,
                provider_metadata: &tool_use.provider_metadata,
            }
            .serialize(serializer),
            Self::StructuredData { media_type, value } => {
                Wire::StructuredData { media_type, value }.serialize(serializer)
            }
        }
    }
}

impl<'de> Deserialize<'de> for SubMessage {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(tag = "type", rename_all = "snake_case")]
        enum Wire {
            Text {
                text: String,
            },
            FileRef {
                attachment_id: String,
                media_type: String,
                #[serde(default)]
                name: Option<String>,
            },
            ToolUse {
                call_id: String,
                tool_name: String,
                arguments: String,
                #[serde(default)]
                provider_metadata: Option<String>,
            },
            StructuredData {
                media_type: String,
                value: String,
            },
        }

        Ok(match Wire::deserialize(deserializer)? {
            Wire::Text { text } => Self::Text { text },
            Wire::FileRef {
                attachment_id,
                media_type,
                name,
            } => Self::FileRef {
                attachment_id,
                media_type,
                name,
            },
            Wire::ToolUse {
                call_id,
                tool_name,
                arguments,
                provider_metadata,
            } => Self::ToolUse(ToolUse {
                call_id,
                tool_name,
                arguments,
                provider_metadata,
            }),
            Wire::StructuredData { media_type, value } => {
                Self::StructuredData { media_type, value }
            }
        })
    }
}

/// An immutable node in a Project's Message forest.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Message {
    /// Message identity.
    pub id: MessageId,
    /// Owning Project.
    pub project_id: ProjectId,
    /// Parent Message, absent only for a root System Message.
    pub parent_message_id: Option<MessageId>,
    /// Provider-facing role.
    pub role: MessageRole,
    /// Ordinary or `ToolResult` protocol kind.
    pub kind: MessageKind,
    /// Creation source.
    pub origin: MessageOrigin,
    /// Ordered content parts.
    pub sub_messages: Vec<SubMessage>,
    /// Session that caused creation, for audit only.
    pub created_by_session_id: Option<SessionId>,
    /// Run provenance, when generated during a Run.
    pub run_id: Option<RunId>,
    /// Monotonic sequence inside `run_id`.
    pub run_seq: Option<u64>,
    /// Fields present only for [`MessageKind::ToolResult`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_result: Option<ToolResult>,
    /// Non-secret extension metadata fixed at creation.
    #[serde(default)]
    pub metadata: DomainMetadata,
    /// Creation time.
    pub created_at: TimestampMs,
}

impl Message {
    /// Validates local Message protocol invariants that do not require storage.
    ///
    /// # Errors
    ///
    /// Returns a stable [`MessageValidationError`] for an invalid root, role,
    /// sub-message kind, Run provenance, or `ToolResult` envelope.
    pub fn validate(&self) -> Result<(), MessageValidationError> {
        if self.parent_message_id.is_none()
            && (self.role != MessageRole::System
                || self.kind != MessageKind::Standard
                || self.run_id.is_some())
        {
            return Err(MessageValidationError::InvalidRootMessage);
        }

        let contains_tool_use = self
            .sub_messages
            .iter()
            .any(|part| matches!(part, SubMessage::ToolUse(_)));
        if contains_tool_use && self.role != MessageRole::Assistant {
            return Err(MessageValidationError::ToolUseRequiresAssistant);
        }

        match (&self.run_id, self.run_seq) {
            (None, None) | (Some(_), Some(1..)) => {}
            _ => return Err(MessageValidationError::InvalidRunProvenance),
        }

        let tool_uses_valid = self.sub_messages.iter().all(|part| match part {
            SubMessage::ToolUse(tool_use) => {
                !tool_use.call_id.is_empty()
                    && !tool_use.tool_name.is_empty()
                    && serde_json::from_str::<serde_json::Value>(&tool_use.arguments).is_ok()
                    && tool_use.provider_metadata.as_ref().is_none_or(|metadata| {
                        serde_json::from_str::<serde_json::Value>(metadata).is_ok()
                    })
            }
            SubMessage::FileRef {
                attachment_id,
                media_type,
                ..
            } => !attachment_id.is_empty() && !media_type.is_empty(),
            SubMessage::StructuredData { media_type, .. } => !media_type.is_empty(),
            SubMessage::Text { .. } => true,
        });
        if !tool_uses_valid {
            return Err(MessageValidationError::InvalidSubMessage);
        }
        let mut call_ids = std::collections::HashSet::new();
        if self.sub_messages.iter().any(|part| {
            matches!(part, SubMessage::ToolUse(tool_use) if !call_ids.insert(&tool_use.call_id))
        }) {
            return Err(MessageValidationError::InvalidSubMessage);
        }

        match self.kind {
            MessageKind::Standard if self.tool_result.is_some() => {
                Err(MessageValidationError::ToolResultMessageInvalid)
            }
            MessageKind::ToolResult if self.role != MessageRole::User => {
                Err(MessageValidationError::ToolResultRequiresUser)
            }
            MessageKind::ToolResult
                if self.origin != MessageOrigin::Tool
                    || self.run_id.is_none()
                    || self.tool_result.is_none()
                    || contains_tool_use
                    || !self.sub_messages.is_empty()
                    || self
                        .tool_result
                        .as_ref()
                        .is_some_and(|result| result.call_id.is_empty()) =>
            {
                Err(MessageValidationError::ToolResultMessageInvalid)
            }
            _ => Ok(()),
        }
    }
}

/// Stable local validation failures for Message construction.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageValidationError {
    /// A root was not a System Message.
    InvalidRootMessage,
    /// A `ToolUse` appeared outside an assistant Message.
    ToolUseRequiresAssistant,
    /// Run identity and positive sequence were not supplied together.
    InvalidRunProvenance,
    /// A sub-message omitted a required protocol field.
    InvalidSubMessage,
    /// A `ToolResult` was not represented as a user Message.
    ToolResultRequiresUser,
    /// `ToolResult` fields, role, origin, or Run provenance were inconsistent.
    ToolResultMessageInvalid,
}

impl std::fmt::Display for MessageValidationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::InvalidRootMessage => "INVALID_ROOT_MESSAGE",
            Self::ToolUseRequiresAssistant => "TOOL_USE_REQUIRES_ASSISTANT",
            Self::InvalidRunProvenance => "INVALID_MESSAGE_RUN_PROVENANCE",
            Self::InvalidSubMessage => "INVALID_SUBMESSAGE_KIND",
            Self::ToolResultRequiresUser => "TOOL_RESULT_REQUIRES_USER",
            Self::ToolResultMessageInvalid => "TOOL_RESULT_MESSAGE_INVALID",
        })
    }
}

impl std::error::Error for MessageValidationError {}

impl From<MessageValidationError> for DomainError {
    fn from(error: MessageValidationError) -> Self {
        let code = match error {
            MessageValidationError::InvalidRootMessage => ErrorCode::InvalidRootMessage,
            MessageValidationError::ToolUseRequiresAssistant => ErrorCode::ToolUseRequiresAssistant,
            MessageValidationError::InvalidRunProvenance => ErrorCode::InvalidMessageRunProvenance,
            MessageValidationError::InvalidSubMessage => ErrorCode::InvalidSubmessageKind,
            MessageValidationError::ToolResultRequiresUser => ErrorCode::ToolResultRequiresUser,
            MessageValidationError::ToolResultMessageInvalid => ErrorCode::ToolResultMessageInvalid,
        };
        Self::invariant(code, error.to_string())
    }
}

/// Storage projection of an immutable Message and its independent visibility.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct StoredMessage {
    /// Immutable Message record.
    pub message: Message,
    /// Whether content must be hidden from projections.
    pub redacted: bool,
}

/// Safe Message representation returned in a path or Session view.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[allow(clippy::large_enum_variant)]
pub enum ProjectedMessage {
    /// Full immutable content is visible.
    Visible(Message),
    /// Content is hidden while identity and graph position remain visible.
    Redacted {
        /// Message identity retained for graph continuity.
        id: MessageId,
        /// Owning Project.
        project_id: ProjectId,
        /// Parent edge retained for graph continuity.
        parent_message_id: Option<MessageId>,
        /// Role retained so protocol ordering remains interpretable.
        role: MessageRole,
    },
}

impl From<StoredMessage> for ProjectedMessage {
    fn from(stored: StoredMessage) -> Self {
        if stored.redacted {
            Self::Redacted {
                id: stored.message.id,
                project_id: stored.message.project_id,
                parent_message_id: stored.message.parent_message_id,
                role: stored.message.role,
            }
        } else {
            Self::Visible(stored.message)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(role: MessageRole) -> Message {
        Message {
            id: MessageId::new("message-1"),
            project_id: ProjectId::new("project-1"),
            parent_message_id: Some(MessageId::new("parent-1")),
            role,
            kind: MessageKind::Standard,
            origin: match role {
                MessageRole::User => MessageOrigin::Human,
                MessageRole::System => MessageOrigin::System,
                MessageRole::Assistant => MessageOrigin::Agent,
            },
            sub_messages: Vec::new(),
            created_by_session_id: None,
            run_id: None,
            run_seq: None,
            tool_result: None,
            metadata: DomainMetadata::default(),
            created_at: TimestampMs(1),
        }
    }

    fn tool_use() -> SubMessage {
        SubMessage::ToolUse(ToolUse {
            call_id: "call-1".into(),
            tool_name: "read_file".into(),
            arguments: r#"{"path":"README.md"}"#.into(),
            provider_metadata: None,
        })
    }

    #[test]
    fn tool_use_only_belongs_to_assistant_messages() {
        let mut assistant = message(MessageRole::Assistant);
        assistant.sub_messages.push(tool_use());
        assistant.validate().unwrap();

        let mut user = message(MessageRole::User);
        user.sub_messages.push(tool_use());
        assert_eq!(
            user.validate().unwrap_err(),
            MessageValidationError::ToolUseRequiresAssistant
        );
    }

    #[test]
    fn tool_result_is_a_special_empty_user_message() {
        let mut result = message(MessageRole::User);
        result.kind = MessageKind::ToolResult;
        result.origin = MessageOrigin::Tool;
        result.run_id = Some(RunId::new("run-1"));
        result.run_seq = Some(2);
        result.tool_result = Some(ToolResult {
            call_id: "call-1".into(),
            status: ToolResultStatus::Succeeded,
            output: Some("ok".into()),
            error: None,
        });
        result.validate().unwrap();

        result.sub_messages.push(SubMessage::Text {
            text: "not allowed".into(),
        });
        assert_eq!(
            result.validate().unwrap_err(),
            MessageValidationError::ToolResultMessageInvalid
        );
    }

    #[test]
    fn sub_message_wire_shape_matches_sqlite_projection() {
        let encoded = serde_json::to_string(&tool_use()).unwrap();
        assert_eq!(
            encoded,
            r#"{"type":"tool_use","call_id":"call-1","tool_name":"read_file","arguments":"{\"path\":\"README.md\"}","provider_metadata":null}"#
        );
        let decoded: SubMessage = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, tool_use());
    }

    #[test]
    fn run_identity_and_sequence_are_atomic() {
        let mut candidate = message(MessageRole::Assistant);
        candidate.run_id = Some(RunId::new("run-1"));
        assert_eq!(
            candidate.validate().unwrap_err(),
            MessageValidationError::InvalidRunProvenance
        );
    }
}
