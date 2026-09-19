use serde::{Deserialize, Serialize};

use crate::DomainMetadata;

/// Stable machine-readable domain failure code.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ErrorCode {
    /// Project path does not exist.
    ProjectPathNotFound,
    /// Project path is not a directory.
    ProjectPathNotDirectory,
    /// Canonical path is already registered.
    ProjectPathAlreadyRegistered,
    /// A requested new Project directory already exists.
    ProjectPathAlreadyExists,
    /// The host's default parent directory cannot be resolved or accessed.
    ProjectDefaultDirectoryUnavailable,
    /// A new Project directory could not be created.
    ProjectDirectoryCreationFailed,
    /// Git initialization failed.
    ProjectGitInitFailed,
    /// Project Git worktree or index contains changes.
    ProjectGitDirty,
    /// Project repository has no readable HEAD commit.
    ProjectGitHeadUnavailable,
    /// Another process owns the Project workspace write lease.
    ProjectWorkspaceBusy,
    /// A file operation escaped the Project boundary.
    ProjectPathOutOfScope,
    /// Project aggregate fields are inconsistent.
    InvalidProject,
    /// Session does not exist.
    SessionNotFound,
    /// Session already follows a non-terminal Run.
    SessionBusy,
    /// Session compare-and-swap failed.
    SessionPointerConflict,
    /// Session and Message belong to different Projects.
    SessionMessageProjectMismatch,
    /// Session aggregate fields are inconsistent.
    InvalidSession,
    /// Message does not exist.
    MessageNotFound,
    /// Message and parent belong to different Projects.
    MessageProjectMismatch,
    /// Root Message is invalid.
    InvalidRootMessage,
    /// Message role is invalid for the operation.
    InvalidMessageRole,
    /// Message UUID is nil or otherwise unusable as an identity.
    InvalidMessageId,
    /// A sub-message is invalid for its containing Message.
    InvalidSubmessageKind,
    /// Run identity and sequence were not supplied together on a Message.
    InvalidMessageRunProvenance,
    /// Immutable Message mutation was attempted.
    MessageImmutable,
    /// `ToolUse` appeared outside an assistant Message.
    ToolUseRequiresAssistant,
    /// `ToolResult` appeared outside a user Message.
    ToolResultRequiresUser,
    /// `ToolResult` envelope is inconsistent.
    ToolResultMessageInvalid,
    /// An interactive human Message omitted its Git HEAD snapshot.
    HumanMessageGitCommitRequired,
    /// Git HEAD provenance appeared on a non-human Message.
    MessageGitCommitNotAllowed,
    /// A Message carried a malformed Git object identity.
    InvalidMessageGitCommit,
    /// Agent does not exist.
    AgentNotFound,
    /// Agent is disabled.
    AgentDisabled,
    /// Agent revision does not exist.
    AgentRevisionNotFound,
    /// Agent lacks a required capability.
    AgentCapabilityUnsupported,
    /// Agent or revision configuration is invalid.
    InvalidAgentConfiguration,
    /// Core configuration or settings are invalid.
    InvalidConfiguration,
    /// Provider invocation failed after adapter normalization.
    ProviderFailed,
    /// Codex Thread enumeration failed.
    CodexHistoryListFailed,
    /// Codex Thread history could not be read completely.
    CodexHistoryReadFailed,
    /// Codex app-server schema lacks a required history capability.
    CodexHistorySchemaUnsupported,
    /// Codex history is incomplete and cannot be published.
    CodexHistoryIncomplete,
    /// A native Thread identity conflicts with an existing binding.
    CodexThreadIdConflict,
    /// A native Turn identity conflicts with immutable local history.
    CodexTurnIdConflict,
    /// A native Thread cannot be assigned to an Ait Project.
    CodexThreadProjectUnbound,
    /// More than one Ait Project matches a native Thread.
    CodexThreadProjectAmbiguous,
    /// A native Thread is already bound to another Project or Session.
    CodexThreadBindingConflict,
    /// Immutable history reconciliation failed its precondition.
    CodexHistoryReconcileConflict,
    /// A provider pagination cursor repeated within one scan.
    CodexHistoryCursorRepeated,
    /// A writable operation requires a newer complete synchronization.
    CodexThreadNotSynced,
    /// Another client owns the active native Turn.
    CodexThreadActiveElsewhere,
    /// Another app-server process owns the Thread writer.
    CodexThreadWriterBusy,
    /// The imported Thread depends on an unsupported native capability.
    CodexThreadCapabilityUnsupported,
    /// Native fork cannot represent the selected Message boundary.
    CodexForkBoundaryUnsupported,
    /// app-server explicitly rejected the pending input.
    CodexInputNotAccepted,
    /// Transport failure left input acceptance indeterminate.
    CodexInputOutcomeUnknown,
    /// Provider history cannot uniquely correlate a pending input.
    CodexInputCorrelationFailed,
    /// Run aggregate fields are inconsistent.
    InvalidRun,
    /// Run cannot be resumed from its current state.
    RunNotResumable,
    /// Run is terminal.
    RunAlreadyTerminal,
    /// Retry allowance is exhausted.
    RunRetryExhausted,
    /// Run recovery failed.
    RunRecoveryFailed,
    /// Run queue compare-and-swap failed.
    RunQueueConflict,
    /// Run was cancelled.
    RunCancelled,
    /// Run exceeded a configured limit.
    RunLimitExceeded,
    /// `ToolUse` cannot be found on the Run path.
    ToolUseNotFound,
    /// Tool call identity is duplicated in a Run.
    ToolCallDuplicate,
    /// `ToolResult` already exists for the call.
    ToolResultDuplicate,
    /// Tool execution and Message belong to different Runs.
    ToolRunMismatch,
    /// Tool execution requires approval.
    ToolApprovalRequired,
    /// Tool execution failed.
    ToolExecutionFailed,
    /// Tool execution aggregate fields are inconsistent.
    InvalidToolExecution,
    /// Cron's Project cannot be used.
    CronProjectUnavailable,
    /// Cron's fixed base Message cannot be used.
    CronBaseMessageUnavailable,
    /// Cron's fixed Agent cannot be used.
    CronAgentUnavailable,
    /// The same Cron occurrence was already claimed.
    CronDuplicateFire,
    /// Cron concurrency policy blocked the occurrence.
    CronConcurrencyBlocked,
    /// Cron configuration is invalid.
    InvalidCron,
}

impl ErrorCode {
    /// Returns the stable wire representation.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ProjectPathNotFound => "PROJECT_PATH_NOT_FOUND",
            Self::ProjectPathNotDirectory => "PROJECT_PATH_NOT_DIRECTORY",
            Self::ProjectPathAlreadyRegistered => "PROJECT_PATH_ALREADY_REGISTERED",
            Self::ProjectPathAlreadyExists => "PROJECT_PATH_ALREADY_EXISTS",
            Self::ProjectDefaultDirectoryUnavailable => "PROJECT_DEFAULT_DIRECTORY_UNAVAILABLE",
            Self::ProjectDirectoryCreationFailed => "PROJECT_DIRECTORY_CREATION_FAILED",
            Self::ProjectGitInitFailed => "PROJECT_GIT_INIT_FAILED",
            Self::ProjectGitDirty => "PROJECT_GIT_DIRTY",
            Self::ProjectGitHeadUnavailable => "PROJECT_GIT_HEAD_UNAVAILABLE",
            Self::ProjectWorkspaceBusy => "PROJECT_WORKSPACE_BUSY",
            Self::ProjectPathOutOfScope => "PROJECT_PATH_OUT_OF_SCOPE",
            Self::InvalidProject => "INVALID_PROJECT",
            Self::SessionNotFound => "SESSION_NOT_FOUND",
            Self::SessionBusy => "SESSION_BUSY",
            Self::SessionPointerConflict => "SESSION_POINTER_CONFLICT",
            Self::SessionMessageProjectMismatch => "SESSION_MESSAGE_PROJECT_MISMATCH",
            Self::InvalidSession => "INVALID_SESSION",
            Self::MessageNotFound => "MESSAGE_NOT_FOUND",
            Self::MessageProjectMismatch => "MESSAGE_PROJECT_MISMATCH",
            Self::InvalidRootMessage => "INVALID_ROOT_MESSAGE",
            Self::InvalidMessageRole => "INVALID_MESSAGE_ROLE",
            Self::InvalidMessageId => "INVALID_MESSAGE_ID",
            Self::InvalidSubmessageKind => "INVALID_SUBMESSAGE_KIND",
            Self::InvalidMessageRunProvenance => "INVALID_MESSAGE_RUN_PROVENANCE",
            Self::MessageImmutable => "MESSAGE_IMMUTABLE",
            Self::ToolUseRequiresAssistant => "TOOL_USE_REQUIRES_ASSISTANT",
            Self::ToolResultRequiresUser => "TOOL_RESULT_REQUIRES_USER",
            Self::ToolResultMessageInvalid => "TOOL_RESULT_MESSAGE_INVALID",
            Self::HumanMessageGitCommitRequired => "HUMAN_MESSAGE_GIT_COMMIT_REQUIRED",
            Self::MessageGitCommitNotAllowed => "MESSAGE_GIT_COMMIT_NOT_ALLOWED",
            Self::InvalidMessageGitCommit => "INVALID_MESSAGE_GIT_COMMIT",
            Self::AgentNotFound => "AGENT_NOT_FOUND",
            Self::AgentDisabled => "AGENT_DISABLED",
            Self::AgentRevisionNotFound => "AGENT_REVISION_NOT_FOUND",
            Self::AgentCapabilityUnsupported => "AGENT_CAPABILITY_UNSUPPORTED",
            Self::InvalidAgentConfiguration => "INVALID_AGENT_CONFIGURATION",
            Self::InvalidConfiguration => "INVALID_CONFIGURATION",
            Self::ProviderFailed => "PROVIDER_FAILED",
            Self::CodexHistoryListFailed => "CODEX_HISTORY_LIST_FAILED",
            Self::CodexHistoryReadFailed => "CODEX_HISTORY_READ_FAILED",
            Self::CodexHistorySchemaUnsupported => "CODEX_HISTORY_SCHEMA_UNSUPPORTED",
            Self::CodexHistoryIncomplete => "CODEX_HISTORY_INCOMPLETE",
            Self::CodexThreadIdConflict => "CODEX_THREAD_ID_CONFLICT",
            Self::CodexTurnIdConflict => "CODEX_TURN_ID_CONFLICT",
            Self::CodexThreadProjectUnbound => "CODEX_THREAD_PROJECT_UNBOUND",
            Self::CodexThreadProjectAmbiguous => "CODEX_THREAD_PROJECT_AMBIGUOUS",
            Self::CodexThreadBindingConflict => "CODEX_THREAD_BINDING_CONFLICT",
            Self::CodexHistoryReconcileConflict => "CODEX_HISTORY_RECONCILE_CONFLICT",
            Self::CodexHistoryCursorRepeated => "CODEX_HISTORY_CURSOR_REPEATED",
            Self::CodexThreadNotSynced => "CODEX_THREAD_NOT_SYNCED",
            Self::CodexThreadActiveElsewhere => "CODEX_THREAD_ACTIVE_ELSEWHERE",
            Self::CodexThreadWriterBusy => "CODEX_THREAD_WRITER_BUSY",
            Self::CodexThreadCapabilityUnsupported => "CODEX_THREAD_CAPABILITY_UNSUPPORTED",
            Self::CodexForkBoundaryUnsupported => "CODEX_FORK_BOUNDARY_UNSUPPORTED",
            Self::CodexInputNotAccepted => "CODEX_INPUT_NOT_ACCEPTED",
            Self::CodexInputOutcomeUnknown => "CODEX_INPUT_OUTCOME_UNKNOWN",
            Self::CodexInputCorrelationFailed => "CODEX_INPUT_CORRELATION_FAILED",
            Self::InvalidRun => "INVALID_RUN",
            Self::RunNotResumable => "RUN_NOT_RESUMABLE",
            Self::RunAlreadyTerminal => "RUN_ALREADY_TERMINAL",
            Self::RunRetryExhausted => "RUN_RETRY_EXHAUSTED",
            Self::RunRecoveryFailed => "RUN_RECOVERY_FAILED",
            Self::RunQueueConflict => "RUN_QUEUE_CONFLICT",
            Self::RunCancelled => "RUN_CANCELLED",
            Self::RunLimitExceeded => "RUN_LIMIT_EXCEEDED",
            Self::ToolUseNotFound => "TOOL_USE_NOT_FOUND",
            Self::ToolCallDuplicate => "TOOL_CALL_DUPLICATE",
            Self::ToolResultDuplicate => "TOOL_RESULT_DUPLICATE",
            Self::ToolRunMismatch => "TOOL_RUN_MISMATCH",
            Self::ToolApprovalRequired => "TOOL_APPROVAL_REQUIRED",
            Self::ToolExecutionFailed => "TOOL_EXECUTION_FAILED",
            Self::InvalidToolExecution => "INVALID_TOOL_EXECUTION",
            Self::CronProjectUnavailable => "CRON_PROJECT_UNAVAILABLE",
            Self::CronBaseMessageUnavailable => "CRON_BASE_MESSAGE_UNAVAILABLE",
            Self::CronAgentUnavailable => "CRON_AGENT_UNAVAILABLE",
            Self::CronDuplicateFire => "CRON_DUPLICATE_FIRE",
            Self::CronConcurrencyBlocked => "CRON_CONCURRENCY_BLOCKED",
            Self::InvalidCron => "INVALID_CRON",
        }
    }
}

impl std::fmt::Display for ErrorCode {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Error envelope shared by domain, application, adapter, IPC, and UI layers.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DomainError {
    /// Stable machine-readable code.
    pub code: ErrorCode,
    /// Safe, human-readable explanation.
    pub message: String,
    /// Whether retrying the same idempotent operation may succeed.
    pub retryable: bool,
    /// Optional non-secret structured context.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<DomainMetadata>,
    /// Optional opaque identifier used to correlate a lower-level cause.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cause_id: Option<String>,
}

impl DomainError {
    /// Creates a non-retryable invariant or input failure.
    #[must_use]
    pub fn invariant(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            retryable: false,
            details: None,
            cause_id: None,
        }
    }

    /// Creates a retryable operational failure.
    ///
    /// Callers must preserve the operation's idempotency key when retrying.
    #[must_use]
    pub fn transient(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            retryable: true,
            details: None,
            cause_id: None,
        }
    }

    /// Adds structured, non-secret details.
    #[must_use]
    pub fn with_details(mut self, details: DomainMetadata) -> Self {
        self.details = Some(details);
        self
    }

    /// Adds an opaque lower-level cause identifier.
    #[must_use]
    pub fn with_cause_id(mut self, cause_id: impl Into<String>) -> Self {
        self.cause_id = Some(cause_id.into());
        self
    }
}

impl std::fmt::Display for DomainError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for DomainError {}

#[cfg(test)]
mod tests;
