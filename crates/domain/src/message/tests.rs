use super::*;

fn message(role: MessageRole) -> Message {
    Message {
        id: MessageId::from_u128(1),
        project_id: ProjectId::new("project-1"),
        parent_message_id: Some(MessageId::from_u128(2)),
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
        git_commit: (role == MessageRole::User).then(|| GitCommit::parse("a".repeat(40)).unwrap()),
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
    result.git_commit = None;
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
        concat!(
            r#"{"type":"tool_use","call_id":"call-1","tool_name":"read_file","#,
            r#""arguments":"{\"path\":\"README.md\"}","provider_metadata":null}"#,
        )
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

#[test]
fn human_user_message_requires_exclusive_valid_git_provenance() {
    let mut user = message(MessageRole::User);
    user.validate().unwrap();

    user.git_commit = None;
    assert_eq!(
        user.validate().unwrap_err(),
        MessageValidationError::HumanMessageGitCommitRequired
    );

    let mut assistant = message(MessageRole::Assistant);
    assistant.git_commit = Some(GitCommit::parse("b".repeat(40)).unwrap());
    assert_eq!(
        assistant.validate().unwrap_err(),
        MessageValidationError::GitCommitNotAllowed
    );

    assert!(serde_json::from_str::<GitCommit>("\"short\"").is_err());
}
