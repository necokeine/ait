use super::*;
use crate::{DomainMetadata, MessageOrigin, MessageRole, ProjectId, ToolResult, ToolResultStatus};

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
        git_commit: None,
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
