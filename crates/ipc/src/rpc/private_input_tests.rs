use super::*;
use ait_contracts::worker::model;

fn assistant_message(arguments: String) -> model::Message {
    model::Message {
        id: "message".into(),
        project_id: "project".into(),
        parent_message_id: Some("parent".into()),
        role: model::MessageRole::Assistant,
        kind: model::MessageKind::Standard,
        origin: model::MessageOrigin::Agent,
        sub_messages: vec![model::SubMessage::ToolUse(model::ToolUse {
            call_id: "private-input".into(),
            tool_name: "write".into(),
            arguments,
            provider_metadata: None,
        })],
        created_by_session_id: None,
        run_id: Some("run".into()),
        run_seq: Some(1),
        tool_result: None,
        git_commit: None,
        metadata: std::collections::BTreeMap::new(),
        created_at: 1,
    }
}

#[test]
fn daemon_ipc_rejects_malformed_messages_and_oversized_tool_intents() {
    let malformed_secret = "NEC248_MALFORMED_IPC_SECRET";
    let malformed = assistant_message(format!(r#"{{"content":"{malformed_secret}""#));
    let error = validate_wire_message_tool_inputs(&malformed).unwrap_err();
    assert_eq!(error, ProtocolError::InvalidTransition);
    assert!(!error.to_string().contains(malformed_secret));

    let oversized_secret = "NEC248_OVERSIZED_IPC_SECRET";
    let tool = model::ToolExecution {
        id: "tool".into(),
        run_id: "run".into(),
        call_id: "private-input".into(),
        assistant_message_id: "message".into(),
        tool_use_index: 0,
        tool_result_message_id: None,
        tool_name: "write".into(),
        arguments: serde_json::json!({
            "content": format!(
                "{oversized_secret}{}",
                "x".repeat(ait_contracts::sensitive::MAX_PRIVATE_TOOL_ARGUMENT_BYTES)
            )
        }),
        attempt: 1,
        approval_status: model::ToolApprovalStatus::NotRequired,
        status: model::ToolExecutionStatus::Pending,
        result: None,
        error: None,
        started_at: None,
        ended_at: None,
        created_at: 1,
    };
    let error = validate_wire_tool_input(&tool).unwrap_err();
    assert_eq!(error, ProtocolError::InvalidTransition);
    assert!(!error.to_string().contains(oversized_secret));
}
