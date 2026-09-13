//! Large Message-tree path baseline using the production traversal policy.
#![allow(missing_docs)]
use ait_domain::{
    DomainMetadata, GitCommit, Message, MessageId, MessageKind, MessageOrigin, MessageRole,
    ProjectId, SubMessage, TimestampMs,
};
use criterion::{Criterion, criterion_group, criterion_main};
use std::{collections::HashMap, hint::black_box};
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
                git_commit: (index > 1).then(|| GitCommit::parse("a".repeat(40)).unwrap()),
                metadata: DomainMetadata::default(),
                created_at: TimestampMs(i64::try_from(index).unwrap()),
            },
        );
    }
    let head = MessageId::from_u128(DEPTH);
    c.bench_function("message_path/10k_depth", |b| {
        b.iter(|| {
            ait_domain::message_path::message_path(black_box(head), |id| messages.get(id)).unwrap()
        });
    });
}

criterion_group!(benches, message_path);
criterion_main!(benches);
