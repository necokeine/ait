use super::*;

fn response(arguments: String) -> AgentResponse {
    AgentResponse {
        sub_messages: vec![SubMessage::ToolUse(ait_domain::ToolUse {
            call_id: "private-input".into(),
            tool_name: "write".into(),
            arguments,
            provider_metadata: None,
        })],
        usage: ait_domain::RunUsage::default(),
    }
}

#[test]
fn worker_rejects_malformed_and_oversized_arguments_without_echoing_them() {
    let malformed_secret = "NEC248_MALFORMED_WORKER_SECRET";
    let malformed = format!(r#"{{"content":"{malformed_secret}""#);
    let oversized_secret = "NEC248_OVERSIZED_WORKER_SECRET";
    let oversized = format!(
        r#"{{"content":"{oversized_secret}{}"}}"#,
        "x".repeat(ait_contracts::sensitive::MAX_PRIVATE_TOOL_ARGUMENT_BYTES)
    );

    for (secret, arguments) in [(malformed_secret, malformed), (oversized_secret, oversized)] {
        let error = validate_tool_arguments(&response(arguments)).unwrap_err();
        assert_eq!(error.code, ErrorCode::InvalidSubmessageKind);
        assert!(!error.to_string().contains(secret));
    }
}
