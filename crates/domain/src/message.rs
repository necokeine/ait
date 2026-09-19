use std::collections::{HashMap, HashSet};
use std::hash::BuildHasher;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    DomainError, DomainMetadata, ErrorCode, GitCommit, InstructionSnapshot, ProjectId, RunId,
    SessionId, TimestampMs,
};

/// Stable identity of an immutable Message.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MessageId(Uuid);

impl MessageId {
    /// Creates an identity from an externally assigned UUID.
    #[must_use]
    pub const fn new(value: Uuid) -> Self {
        Self(value)
    }

    /// Creates an identity from its raw UUID value.
    #[must_use]
    pub const fn from_u128(value: u128) -> Self {
        Self(Uuid::from_u128(value))
    }

    /// Parses a UUID string.
    ///
    /// # Errors
    ///
    /// Returns [`uuid::Error`] when the input is not a UUID.
    pub fn parse(value: &str) -> Result<Self, uuid::Error> {
        Uuid::parse_str(value).map(Self)
    }

    /// Returns the underlying UUID.
    #[must_use]
    pub const fn as_uuid(&self) -> &Uuid {
        &self.0
    }
}

impl From<Uuid> for MessageId {
    fn from(value: Uuid) -> Self {
        Self::new(value)
    }
}

impl std::fmt::Display for MessageId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(formatter)
    }
}

/// A typed component stored in an immutable System Message.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SystemMessageComponent {
    /// Project instruction sources captured at a particular revision.
    ProjectInstructions(InstructionSnapshot),
}

/// Immutable root System Message for a new Message tree.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SystemMessage {
    /// Message identity.
    pub id: MessageId,
    /// Owning Project.
    pub project_id: ProjectId,
    /// Structured snapshots used later to assemble a provider prompt.
    pub components: Vec<SystemMessageComponent>,
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
    /// Content imported from an authoritative provider history.
    Provider,
}

/// One provider-native history item retained without adopting Ait's tool protocol.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ProviderItem {
    /// Stable provider kind, for example `codex`.
    pub provider_kind: String,
    /// Provider-assigned item identity.
    pub external_item_id: String,
    /// Provider-native item discriminator.
    pub item_type: String,
    /// Zero-based position in the provider Turn's final item snapshot.
    pub ordinal: u32,
    /// Bounded provider payload after the adapter's sensitive-field policy.
    pub payload: serde_json::Value,
    /// Version of the payload normalization contract.
    pub payload_schema_version: u32,
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
    /// Provider-native history item that is not an Ait `ToolUse` or `ToolResult`.
    ProviderItem(ProviderItem),
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
            ProviderItem {
                provider_kind: &'a str,
                external_item_id: &'a str,
                item_type: &'a str,
                ordinal: u32,
                payload: &'a serde_json::Value,
                payload_schema_version: u32,
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
            Self::ProviderItem(item) => Wire::ProviderItem {
                provider_kind: &item.provider_kind,
                external_item_id: &item.external_item_id,
                item_type: &item.item_type,
                ordinal: item.ordinal,
                payload: &item.payload,
                payload_schema_version: item.payload_schema_version,
            }
            .serialize(serializer),
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
            ProviderItem {
                provider_kind: String,
                external_item_id: String,
                item_type: String,
                ordinal: u32,
                payload: serde_json::Value,
                payload_schema_version: u32,
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
            Wire::ProviderItem {
                provider_kind,
                external_item_id,
                item_type,
                ordinal,
                payload,
                payload_schema_version,
            } => Self::ProviderItem(ProviderItem {
                provider_kind,
                external_item_id,
                item_type,
                ordinal,
                payload,
                payload_schema_version,
            }),
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
    /// Clean repository HEAD captured for an interactive human user Message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_commit: Option<GitCommit>,
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
        if self.id.as_uuid().is_nil() {
            return Err(MessageValidationError::InvalidMessageId);
        }
        if self
            .git_commit
            .as_ref()
            .is_some_and(|commit| !commit.is_valid())
        {
            return Err(MessageValidationError::InvalidGitCommit);
        }
        if self.parent_message_id.is_none()
            && (self.role != MessageRole::System
                || self.kind != MessageKind::Standard
                || self.run_id.is_some())
        {
            return Err(MessageValidationError::InvalidRootMessage);
        }

        let contains_tool_use = validate_sub_message_roles(self.role, &self.sub_messages)?;

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
            SubMessage::ProviderItem(item) => {
                !item.provider_kind.is_empty()
                    && !item.external_item_id.is_empty()
                    && !item.item_type.is_empty()
                    && item.payload_schema_version > 0
            }
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
            _ if self.role == MessageRole::User
                && self.kind == MessageKind::Standard
                && self.origin == MessageOrigin::Human
                && self.git_commit.is_none()
                && !is_native_codex_human_input(&self.metadata) =>
            {
                Err(MessageValidationError::HumanMessageGitCommitRequired)
            }
            _ if self.git_commit.is_some()
                && (self.role != MessageRole::User
                    || self.kind != MessageKind::Standard
                    || self.origin != MessageOrigin::Human) =>
            {
                Err(MessageValidationError::GitCommitNotAllowed)
            }
            _ => Ok(()),
        }
    }
}

fn validate_sub_message_roles(
    role: MessageRole,
    sub_messages: &[SubMessage],
) -> Result<bool, MessageValidationError> {
    let contains_tool_use = sub_messages
        .iter()
        .any(|part| matches!(part, SubMessage::ToolUse(_)));
    if contains_tool_use && role != MessageRole::Assistant {
        return Err(MessageValidationError::ToolUseRequiresAssistant);
    }
    if role != MessageRole::Assistant
        && sub_messages
            .iter()
            .any(|part| matches!(part, SubMessage::ProviderItem(_)))
    {
        return Err(MessageValidationError::InvalidSubMessage);
    }
    Ok(contains_tool_use)
}

fn is_native_codex_human_input(metadata: &DomainMetadata) -> bool {
    let Some(codex) = metadata.0.get("codex") else {
        return false;
    };
    codex
        .get("submitted_via")
        .and_then(serde_json::Value::as_str)
        == Some("ait")
        && codex
            .get("workspace_mode")
            .and_then(serde_json::Value::as_str)
            == Some("native_cwd")
        && ["provider_id", "thread_id"].iter().all(|field| {
            codex
                .get(*field)
                .and_then(serde_json::Value::as_str)
                .is_some_and(|value| !value.is_empty())
        })
}

/// Resolves one root-to-head path, checking cycles, ownership, and the root role.
///
/// # Errors
///
/// Returns a stable domain error for a missing node, cycle, cross-Project parent,
/// or invalid root.
pub fn message_path<S: BuildHasher>(
    head: MessageId,
    messages: &HashMap<MessageId, Message, S>,
) -> Result<Vec<&Message>, DomainError> {
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
        let message = messages.get(&id).ok_or_else(|| {
            DomainError::invariant(ErrorCode::MessageNotFound, "message path is incomplete")
        })?;
        if project.is_some_and(|project_id| project_id != &message.project_id) {
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
    if path
        .first()
        .is_none_or(|message| message.role != MessageRole::System)
    {
        return Err(DomainError::invariant(
            ErrorCode::InvalidMessageRole,
            "message path must begin with a system root",
        ));
    }
    Ok(path)
}

/// Stable local validation failures for Message construction.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageValidationError {
    /// A nil UUID was supplied as a Message identity.
    InvalidMessageId,
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
    /// A human user Message omitted its clean repository HEAD snapshot.
    HumanMessageGitCommitRequired,
    /// Git provenance was attached to a Message that is not human user input.
    GitCommitNotAllowed,
    /// The attached Git object identity was not a full SHA-1 or SHA-256 hash.
    InvalidGitCommit,
}

impl std::fmt::Display for MessageValidationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::InvalidMessageId => "INVALID_MESSAGE_ID",
            Self::InvalidRootMessage => "INVALID_ROOT_MESSAGE",
            Self::ToolUseRequiresAssistant => "TOOL_USE_REQUIRES_ASSISTANT",
            Self::InvalidRunProvenance => "INVALID_MESSAGE_RUN_PROVENANCE",
            Self::InvalidSubMessage => "INVALID_SUBMESSAGE_KIND",
            Self::ToolResultRequiresUser => "TOOL_RESULT_REQUIRES_USER",
            Self::ToolResultMessageInvalid => "TOOL_RESULT_MESSAGE_INVALID",
            Self::HumanMessageGitCommitRequired => "HUMAN_MESSAGE_GIT_COMMIT_REQUIRED",
            Self::GitCommitNotAllowed => "MESSAGE_GIT_COMMIT_NOT_ALLOWED",
            Self::InvalidGitCommit => "INVALID_MESSAGE_GIT_COMMIT",
        })
    }
}

impl std::error::Error for MessageValidationError {}

impl From<MessageValidationError> for DomainError {
    fn from(error: MessageValidationError) -> Self {
        let code = match error {
            MessageValidationError::InvalidMessageId => ErrorCode::InvalidMessageId,
            MessageValidationError::InvalidRootMessage => ErrorCode::InvalidRootMessage,
            MessageValidationError::ToolUseRequiresAssistant => ErrorCode::ToolUseRequiresAssistant,
            MessageValidationError::InvalidRunProvenance => ErrorCode::InvalidMessageRunProvenance,
            MessageValidationError::InvalidSubMessage => ErrorCode::InvalidSubmessageKind,
            MessageValidationError::ToolResultRequiresUser => ErrorCode::ToolResultRequiresUser,
            MessageValidationError::ToolResultMessageInvalid => ErrorCode::ToolResultMessageInvalid,
            MessageValidationError::HumanMessageGitCommitRequired => {
                ErrorCode::HumanMessageGitCommitRequired
            }
            MessageValidationError::GitCommitNotAllowed => ErrorCode::MessageGitCommitNotAllowed,
            MessageValidationError::InvalidGitCommit => ErrorCode::InvalidMessageGitCommit,
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

impl MessageRole {
    /// Stable transport spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::System => "system",
            Self::Assistant => "assistant",
        }
    }
}
impl MessageKind {
    /// Stable transport spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Standard => "standard",
            Self::ToolResult => "tool_result",
        }
    }
}

/// Validates text supplied as a new human Message.
/// # Errors
/// Rejects blank input before any Message or Session transition.
pub fn validate_message_text(text: &str) -> Result<(), DomainError> {
    if text.trim().is_empty() {
        return Err(DomainError::invariant(
            ErrorCode::InvalidMessageRole,
            "message text is required",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
