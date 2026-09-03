use crate::{MessageId, ProjectId, SessionId};

/// Stable identity of a Run that produced a Message.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
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
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MessageRole {
    /// Human, scheduler, system-injected, or `ToolResult` input.
    User,
    /// Root instruction snapshot.
    System,
    /// Agent output.
    Assistant,
}

/// Protocol kind of a Message.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MessageKind {
    /// Ordinary user, system, or assistant content.
    Standard,
    /// A special user Message answering a prior `ToolUse`.
    ToolResult,
}

/// Actor or subsystem that created a Message.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
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
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
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
#[derive(Clone, Debug, Eq, PartialEq)]
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

/// One ordered part inside a Message.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SubMessage {
    /// Plain text.
    Text(String),
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
    ToolUse {
        /// Provider-stable call identity.
        call_id: String,
        /// Registered tool name.
        tool_name: String,
        /// Canonical structured arguments.
        arguments: String,
        /// Optional provider-specific, non-secret metadata.
        provider_metadata: Option<String>,
    },
    /// Typed structured content encoded in a canonical representation.
    StructuredData {
        /// Content media type.
        media_type: String,
        /// Canonical encoded value.
        value: String,
    },
}

/// An immutable node in a Project's Message forest.
#[derive(Clone, Debug, Eq, PartialEq)]
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
    pub tool_result: Option<ToolResult>,
}

impl Message {
    /// Validates local Message protocol invariants that do not require storage.
    ///
    /// # Errors
    ///
    /// Returns a stable [`MessageValidationError`] for an invalid root, role,
    /// sub-message kind, Run provenance, or `ToolResult` envelope.
    pub fn validate(&self) -> Result<(), MessageValidationError> {
        if self.parent_message_id.is_none() && self.role != MessageRole::System {
            return Err(MessageValidationError::InvalidRootMessage);
        }

        let contains_tool_use = self
            .sub_messages
            .iter()
            .any(|part| matches!(part, SubMessage::ToolUse { .. }));
        if contains_tool_use && self.role != MessageRole::Assistant {
            return Err(MessageValidationError::ToolUseRequiresAssistant);
        }

        match (&self.run_id, self.run_seq) {
            (None, None) | (Some(_), Some(1..)) => {}
            _ => return Err(MessageValidationError::InvalidRunProvenance),
        }

        match self.kind {
            MessageKind::Standard if self.tool_result.is_some() => {
                Err(MessageValidationError::ToolResultMessageInvalid)
            }
            MessageKind::ToolResult
                if self.role != MessageRole::User
                    || self.origin != MessageOrigin::Tool
                    || self.run_id.is_none()
                    || self.tool_result.is_none() =>
            {
                Err(MessageValidationError::ToolResultMessageInvalid)
            }
            _ => Ok(()),
        }
    }
}

/// Stable local validation failures for Message construction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MessageValidationError {
    /// A root was not a System Message.
    InvalidRootMessage,
    /// A `ToolUse` appeared outside an assistant Message.
    ToolUseRequiresAssistant,
    /// Run identity and positive sequence were not supplied together.
    InvalidRunProvenance,
    /// `ToolResult` fields, role, origin, or Run provenance were inconsistent.
    ToolResultMessageInvalid,
}

impl std::fmt::Display for MessageValidationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::InvalidRootMessage => "INVALID_ROOT_MESSAGE",
            Self::ToolUseRequiresAssistant => "TOOL_USE_REQUIRES_ASSISTANT",
            Self::InvalidRunProvenance => "INVALID_MESSAGE_RUN_PROVENANCE",
            Self::ToolResultMessageInvalid => "TOOL_RESULT_MESSAGE_INVALID",
        })
    }
}

impl std::error::Error for MessageValidationError {}

/// Storage projection of an immutable Message and its independent visibility.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoredMessage {
    /// Immutable Message record.
    pub message: Message,
    /// Whether content must be hidden from projections.
    pub redacted: bool,
}

/// Safe Message representation returned in a path or Session view.
#[derive(Clone, Debug, Eq, PartialEq)]
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
