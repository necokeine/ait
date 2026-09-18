use super::*;
use ait_domain::{MessageKind, MessageOrigin, MessageRole};

fn assistant_message(arguments: String) -> Message {
    Message {
        id: MessageId::from_u128(2),
        project_id: ait_domain::ProjectId::new("project"),
        parent_message_id: Some(MessageId::from_u128(1)),
        role: MessageRole::Assistant,
        kind: MessageKind::Standard,
        origin: MessageOrigin::Agent,
        sub_messages: vec![SubMessage::ToolUse(ait_domain::ToolUse {
            call_id: "private-input".into(),
            tool_name: "write".into(),
            arguments,
            provider_metadata: None,
        })],
        created_by_session_id: None,
        run_id: Some(RunId::new("run")),
        run_seq: Some(1),
        tool_result: None,
        git_commit: None,
        metadata: ait_domain::DomainMetadata::default(),
        created_at: ait_domain::TimestampMs(1),
    }
}

#[test]
fn application_rejects_malformed_messages_and_oversized_tool_intents() {
    let malformed_secret = "NEC248_MALFORMED_APPLICATION_SECRET";
    let malformed = assistant_message(format!(r#"{{"content":"{malformed_secret}""#));
    let error = validate_message_tool_inputs(&malformed).unwrap_err();
    assert!(!error.to_string().contains(malformed_secret));

    let oversized_secret = "NEC248_OVERSIZED_APPLICATION_SECRET";
    let tool = ToolExecution {
        id: ait_domain::ToolExecutionId::new("tool"),
        run_id: RunId::new("run"),
        call_id: "private-input".into(),
        assistant_message_id: MessageId::from_u128(2),
        tool_use_index: 0,
        tool_result_message_id: None,
        tool_name: "write".into(),
        arguments: json!({
            "content": format!(
                "{oversized_secret}{}",
                "x".repeat(ait_contracts::sensitive::MAX_PRIVATE_TOOL_ARGUMENT_BYTES)
            )
        }),
        attempt: 1,
        approval_status: ait_domain::ToolApprovalStatus::NotRequired,
        status: ait_domain::ToolExecutionStatus::Pending,
        result: None,
        error: None,
        started_at: None,
        ended_at: None,
        created_at: ait_domain::TimestampMs(1),
    };
    let error = validate_execution_tool_input(&tool).unwrap_err();
    assert!(!error.to_string().contains(oversized_secret));
}
