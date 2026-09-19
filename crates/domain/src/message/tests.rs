use super::*;

#[test]
fn message_id_is_a_uuid_with_stable_serde() {
    let id = MessageId::from_u128(1);
    let encoded = serde_json::to_string(&id).unwrap();

    assert_eq!(encoded, "\"00000000-0000-0000-0000-000000000001\"");
    assert_eq!(serde_json::from_str::<MessageId>(&encoded).unwrap(), id);
    assert!(MessageId::parse("message-1").is_err());
}
use std::collections::HashMap;

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
fn provider_item_is_an_assistant_sub_message_without_tool_semantics() {
    let part = SubMessage::ProviderItem(ProviderItem {
        provider_kind: "codex".into(),
        external_item_id: "item-1".into(),
        item_type: "commandExecution".into(),
        ordinal: 2,
        payload: serde_json::json!({"status": "completed"}),
        payload_schema_version: 1,
    });
    let encoded = serde_json::to_value(&part).unwrap();
    assert_eq!(encoded["type"], "provider_item");

    let mut assistant = message(MessageRole::Assistant);
    assistant.origin = MessageOrigin::Provider;
    assistant.sub_messages.push(part.clone());
    assistant.validate().unwrap();
    assert_eq!(serde_json::from_value::<SubMessage>(encoded).unwrap(), part);

    let mut user = message(MessageRole::User);
    user.origin = MessageOrigin::Provider;
    user.sub_messages.push(part);
    assert_eq!(
        user.validate().unwrap_err(),
        MessageValidationError::InvalidSubMessage
    );
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

#[test]
fn native_codex_human_input_can_use_provider_provenance_instead_of_git() {
    let mut user = message(MessageRole::User);
    user.git_commit = None;
    user.metadata.0.insert(
        "codex".into(),
        serde_json::json!({
            "submitted_via": "ait",
            "workspace_mode": "native_cwd",
            "provider_id": "builtin-codex",
            "thread_id": "thread-1",
        }),
    );
    user.validate().unwrap();

    user.metadata.0.get_mut("codex").unwrap()["thread_id"] =
        serde_json::Value::String(String::new());
    assert_eq!(
        user.validate().unwrap_err(),
        MessageValidationError::HumanMessageGitCommitRequired
    );
}

#[test]
fn path_checks_missing_cycles_ownership_and_root_without_mutating_history() {
    let root = message_node(1, None, MessageRole::System);
    let child = message_node(2, Some(1), MessageRole::User);
    let mut messages = HashMap::from([(root.id, root), (child.id, child)]);
    let head = MessageId::from_u128(2);
    let ids: Vec<_> = message_path(head, &messages)
        .expect("valid path")
        .iter()
        .map(|message| message.id)
        .collect();
    assert_eq!(ids, vec![MessageId::from_u128(1), head]);
    assert_eq!(
        message_path(MessageId::from_u128(3), &messages)
            .expect_err("unknown head must fail")
            .code,
        ErrorCode::MessageNotFound
    );

    messages.get_mut(&head).expect("child exists").project_id = ProjectId::new("another");
    assert_eq!(
        message_path(head, &messages)
            .expect_err("cross-project path must fail")
            .code,
        ErrorCode::SessionMessageProjectMismatch
    );

    messages.get_mut(&head).expect("child exists").project_id = ProjectId::new("p");
    messages
        .get_mut(&MessageId::from_u128(1))
        .expect("root exists")
        .parent_message_id = Some(head);
    assert_eq!(
        message_path(head, &messages)
            .expect_err("cycle must fail")
            .code,
        ErrorCode::InvalidMessageId
    );

    let root = messages
        .get_mut(&MessageId::from_u128(1))
        .expect("root exists");
    root.parent_message_id = None;
    root.role = MessageRole::User;
    let original = messages.clone();
    assert_eq!(
        message_path(head, &messages)
            .expect_err("non-system root must fail")
            .code,
        ErrorCode::InvalidMessageRole
    );
    assert_eq!(messages, original);
}

fn message_node(id: u128, parent: Option<u128>, role: MessageRole) -> Message {
    Message {
        id: MessageId::from_u128(id),
        project_id: ProjectId::new("p"),
        parent_message_id: parent.map(MessageId::from_u128),
        role,
        kind: MessageKind::Standard,
        origin: MessageOrigin::Agent,
        sub_messages: Vec::new(),
        created_by_session_id: None,
        run_id: None,
        run_seq: None,
        tool_result: None,
        git_commit: None,
        metadata: DomainMetadata::default(),
        created_at: TimestampMs(0),
    }
}
