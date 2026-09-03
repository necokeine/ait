use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{DomainError, ErrorCode, Message, MessageId, MessageKind, RunId, TimestampMs};

/// Stable identity of one `ToolExecution` attempt.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ToolExecutionId(String);

impl ToolExecutionId {
    /// Creates an externally assigned execution identity.
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

/// Approval lifecycle for a tool execution.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolApprovalStatus {
    /// Policy permits execution without approval.
    NotRequired,
    /// Waiting for an explicit decision.
    Pending,
    /// Explicitly approved.
    Approved,
    /// Explicitly denied.
    Denied,
}

/// Execution lifecycle independent of the Message tree.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolExecutionStatus {
    /// Created but not running.
    Pending,
    /// Tool process or adapter is active.
    Running,
    /// Tool completed successfully.
    Succeeded,
    /// Tool failed.
    Failed,
    /// Approval was denied.
    Denied,
    /// Execution was cancelled.
    Cancelled,
}

impl ToolExecutionStatus {
    /// Returns whether this execution attempt has ended.
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Succeeded | Self::Failed | Self::Denied | Self::Cancelled
        )
    }
}

/// Tool lifecycle and audit record linking an assistant `ToolUse` to its result.
///
/// Full unbounded output belongs in attachment storage; `result` and `error`
/// are bounded safe values suitable for persistence and display.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ToolExecution {
    /// Execution attempt identity.
    pub id: ToolExecutionId,
    /// Owning Run.
    pub run_id: RunId,
    /// Provider-stable `ToolUse` call identity, unique within the Run.
    pub call_id: String,
    /// Assistant Message containing the `ToolUse`.
    pub assistant_message_id: MessageId,
    /// Zero-based index in the Message's ordered sub-messages.
    pub tool_use_index: u32,
    /// Final user `ToolResult` Message, once persisted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_result_message_id: Option<MessageId>,
    /// Registered tool name copied from the `ToolUse`.
    pub tool_name: String,
    /// Canonical structured arguments copied from the `ToolUse`.
    pub arguments: Value,
    /// Attempt number for this `call_id`, beginning at one.
    pub attempt: u32,
    /// Approval lifecycle.
    pub approval_status: ToolApprovalStatus,
    /// Execution lifecycle.
    pub status: ToolExecutionStatus,
    /// Bounded structured result summary.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    /// Safe failure information.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<DomainError>,
    /// Execution start time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<TimestampMs>,
    /// Terminal time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<TimestampMs>,
    /// Record creation time.
    pub created_at: TimestampMs,
}

impl ToolExecution {
    /// Validates lifecycle and approval combinations.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorCode::InvalidToolExecution`] for an impossible record.
    pub fn validate(&self) -> Result<(), DomainError> {
        let pending_shape = self.status == ToolExecutionStatus::Pending
            && self.started_at.is_none()
            && self.ended_at.is_none();
        let running_shape = self.status == ToolExecutionStatus::Running
            && self.started_at.is_some()
            && self.ended_at.is_none();
        let terminal_shape = self.status.is_terminal() && self.ended_at.is_some();
        let timestamps_valid = self.started_at.is_none_or(|time| time >= self.created_at)
            && self
                .ended_at
                .is_none_or(|time| time >= self.started_at.unwrap_or(self.created_at));
        let approval_valid = match self.approval_status {
            ToolApprovalStatus::Pending => self.status == ToolExecutionStatus::Pending,
            ToolApprovalStatus::Denied => self.status == ToolExecutionStatus::Denied,
            ToolApprovalStatus::NotRequired | ToolApprovalStatus::Approved => {
                self.status != ToolExecutionStatus::Denied
            }
        };
        let result_link_valid = self
            .tool_result_message_id
            .as_ref()
            .is_none_or(|_| self.status.is_terminal());

        if self.id.as_str().is_empty()
            || self.run_id.as_str().is_empty()
            || self.call_id.is_empty()
            || self.assistant_message_id.as_uuid().is_nil()
            || self.tool_name.is_empty()
            || self.attempt == 0
            || !(pending_shape || running_shape || terminal_shape)
            || !timestamps_valid
            || !approval_valid
            || !result_link_valid
        {
            return Err(DomainError::invariant(
                ErrorCode::InvalidToolExecution,
                "tool execution identity, approval, lifecycle, or timestamps are inconsistent",
            ));
        }
        Ok(())
    }

    /// Validates a final `ToolResult` Message before linking it to this execution.
    ///
    /// # Errors
    ///
    /// Returns a stable tool error when the Message is not the unique result for
    /// the same call and Run.
    pub fn validate_result_message(&self, message: &Message) -> Result<(), DomainError> {
        message.validate().map_err(DomainError::from)?;
        if self.tool_result_message_id.is_some() {
            return Err(DomainError::invariant(
                ErrorCode::ToolResultDuplicate,
                "tool execution already has a final result message",
            ));
        }
        if message.run_id.as_ref() != Some(&self.run_id) {
            return Err(DomainError::invariant(
                ErrorCode::ToolRunMismatch,
                "tool result message belongs to another run",
            ));
        }
        if message.kind != MessageKind::ToolResult
            || message
                .tool_result
                .as_ref()
                .is_none_or(|result| result.call_id != self.call_id)
        {
            return Err(DomainError::invariant(
                ErrorCode::ToolUseNotFound,
                "tool result does not match this call",
            ));
        }
        let Some(result_status) = message.tool_result.as_ref().map(|result| result.status) else {
            return Err(DomainError::invariant(
                ErrorCode::ToolUseNotFound,
                "tool result does not match this call",
            ));
        };
        let status_matches = matches!(
            (self.status, result_status),
            (
                ToolExecutionStatus::Succeeded,
                crate::ToolResultStatus::Succeeded
            ) | (ToolExecutionStatus::Failed, crate::ToolResultStatus::Failed)
                | (ToolExecutionStatus::Denied, crate::ToolResultStatus::Denied)
                | (
                    ToolExecutionStatus::Cancelled,
                    crate::ToolResultStatus::Cancelled
                )
        );
        if !status_matches {
            return Err(DomainError::invariant(
                ErrorCode::InvalidToolExecution,
                "tool result status does not match the terminal execution status",
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        DomainMetadata, MessageOrigin, MessageRole, ProjectId, ToolResult, ToolResultStatus,
    };

    fn pending() -> ToolExecution {
        ToolExecution {
            id: ToolExecutionId::new("tool-1"),
            run_id: RunId::new("run-1"),
            call_id: "call-1".into(),
            assistant_message_id: MessageId::from_u128(1),
            tool_use_index: 0,
            tool_result_message_id: None,
            tool_name: "read_file".into(),
            arguments: serde_json::json!({"path": "README.md"}),
            attempt: 1,
            approval_status: ToolApprovalStatus::Pending,
            status: ToolExecutionStatus::Pending,
            result: None,
            error: None,
            started_at: None,
            ended_at: None,
            created_at: TimestampMs(10),
        }
    }

    #[test]
    fn approval_and_lifecycle_must_agree() {
        let mut execution = pending();
        execution.validate().unwrap();
        execution.status = ToolExecutionStatus::Running;
        assert_eq!(
            execution.validate().unwrap_err().code,
            ErrorCode::InvalidToolExecution
        );
    }

    #[test]
    fn approval_status_round_trips_as_snake_case() {
        let encoded = serde_json::to_string(&ToolApprovalStatus::NotRequired).unwrap();
        assert_eq!(encoded, "\"not_required\"");
        assert_eq!(
            serde_json::from_str::<ToolApprovalStatus>(&encoded).unwrap(),
            ToolApprovalStatus::NotRequired
        );
    }

    #[test]
    fn final_result_must_match_call_run_and_status() {
        let mut execution = pending();
        execution.approval_status = ToolApprovalStatus::NotRequired;
        execution.status = ToolExecutionStatus::Succeeded;
        execution.started_at = Some(TimestampMs(11));
        execution.ended_at = Some(TimestampMs(12));
        execution.validate().unwrap();
        let mut message = Message {
            id: MessageId::from_u128(2),
            project_id: ProjectId::new("project-1"),
            parent_message_id: Some(MessageId::from_u128(1)),
            role: MessageRole::User,
            kind: MessageKind::ToolResult,
            origin: MessageOrigin::Tool,
            sub_messages: Vec::new(),
            created_by_session_id: None,
            run_id: Some(RunId::new("run-1")),
            run_seq: Some(2),
            tool_result: Some(ToolResult {
                call_id: "call-1".into(),
                status: ToolResultStatus::Succeeded,
                output: Some("ok".into()),
                error: None,
            }),
            metadata: DomainMetadata::default(),
            created_at: TimestampMs(12),
        };
        execution.validate_result_message(&message).unwrap();

        message.run_id = Some(RunId::new("run-2"));
        assert_eq!(
            execution
                .validate_result_message(&message)
                .unwrap_err()
                .code,
            ErrorCode::ToolRunMismatch
        );
    }
}
