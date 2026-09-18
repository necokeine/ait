use ait_domain::{
    DomainMetadata, ErrorCode, Message, MessageId, MessageKind, MessageOrigin, MessageRole,
    ProjectId, RunId, SubMessage, TimestampMs,
};

use crate::control::conversation::MessageRecord;

fn domain_message() -> Message {
    Message {
        id: MessageId::from_u128(2),
        project_id: ProjectId::new("project-1"),
        parent_message_id: Some(MessageId::from_u128(1)),
        role: MessageRole::Assistant,
        kind: MessageKind::Standard,
        origin: MessageOrigin::Agent,
        sub_messages: vec![SubMessage::Text {
            text: "first second".into(),
        }],
        created_by_session_id: None,
        run_id: Some(RunId::new("run-1")),
        run_seq: Some(1),
        tool_result: None,
        git_commit: None,
        metadata: DomainMetadata::default(),
        created_at: TimestampMs(10),
    }
}

#[test]
fn domain_message_round_trips_through_persisted_record() {
    let expected = domain_message();

    let state = MessageRecord::from(expected.clone());
    let actual = Message::try_from(state).expect("generated state must be valid");

    assert_eq!(actual, expected);
}

#[test]
fn native_message_conversion_rejects_projection_drift() {
    let mut state = MessageRecord::from(domain_message());
    state.text = Some("different".into());

    assert_eq!(
        Message::try_from(&state)
            .expect_err("mismatched projection must fail")
            .code,
        ErrorCode::InvalidMessageId
    );
}
