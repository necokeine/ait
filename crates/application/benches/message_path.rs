//! Large Message-tree path-read baseline.
#![allow(missing_docs)]

use std::{collections::HashMap, hint::black_box, sync::Arc};

use ait_application::MessageService;
use ait_domain::{
    DomainMetadata, Message, MessageId, MessageKind, MessageOrigin, MessageRole, ProjectId,
    Session, SessionId, StoredMessage, SubMessage, TimestampMs,
};
use ait_ports::{MessageStore, MessageStoreError, SessionAdvance, SessionStore, SessionStoreError};
use criterion::{Criterion, criterion_group, criterion_main};

struct ReadOnlyMessages(HashMap<MessageId, Message>);

impl MessageStore for ReadOnlyMessages {
    fn append_message(&self, message: Message) -> Result<Message, MessageStoreError> {
        Ok(message)
    }

    fn get_message(&self, id: &MessageId) -> Result<StoredMessage, MessageStoreError> {
        self.0
            .get(id)
            .cloned()
            .map(|message| StoredMessage {
                message,
                redacted: false,
            })
            .ok_or(MessageStoreError::MessageNotFound(*id))
    }
}

struct NoSessions;

impl SessionStore for NoSessions {
    fn create_session(&self, session: Session) -> Result<Session, SessionStoreError> {
        Ok(session)
    }

    fn get_session(&self, id: &SessionId) -> Result<Session, SessionStoreError> {
        Err(SessionStoreError::SessionNotFound(id.clone()))
    }

    fn advance_head(
        &self,
        session_id: &SessionId,
        _expected_head: &MessageId,
        _expected_version: u64,
        _new_head: &MessageId,
    ) -> Result<SessionAdvance, SessionStoreError> {
        Err(SessionStoreError::SessionNotFound(session_id.clone()))
    }
}

fn message_path(c: &mut Criterion) {
    const DEPTH: u128 = 10_000;
    let project_id = ProjectId::new("benchmark-project");
    let mut messages = HashMap::with_capacity(usize::try_from(DEPTH).unwrap());
    for index in 1..=DEPTH {
        let id = MessageId::from_u128(index);
        messages.insert(
            id,
            Message {
                id,
                project_id: project_id.clone(),
                parent_message_id: (index > 1).then(|| MessageId::from_u128(index - 1)),
                role: if index == 1 {
                    MessageRole::System
                } else {
                    MessageRole::User
                },
                kind: MessageKind::Standard,
                origin: if index == 1 {
                    MessageOrigin::Project
                } else {
                    MessageOrigin::Human
                },
                sub_messages: vec![SubMessage::Text { text: "x".into() }],
                created_by_session_id: None,
                run_id: None,
                run_seq: None,
                tool_result: None,
                metadata: DomainMetadata::default(),
                created_at: TimestampMs(i64::try_from(index).unwrap()),
            },
        );
    }
    let service = MessageService::new(Arc::new(ReadOnlyMessages(messages)), Arc::new(NoSessions));
    let head = MessageId::from_u128(DEPTH);
    c.bench_function("message_path/10k_depth", |b| {
        b.iter(|| service.message_path(black_box(&head)).unwrap());
    });
}

criterion_group!(benches, message_path);
criterion_main!(benches);
