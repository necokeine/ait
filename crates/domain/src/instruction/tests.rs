use super::*;

fn instruction_source(priority: u32) -> InstructionSourceSnapshot {
    InstructionSourceSnapshot {
        summary: InstructionSourceSummary {
            name: format!("source-{priority}"),
            locator: format!("source-{priority}.md"),
            priority,
            content_digest: "a".repeat(64),
            byte_len: 4,
        },
        content: "test".into(),
    }
}

#[test]
fn instruction_priorities_must_be_strictly_increasing() {
    let mut snapshot = InstructionSnapshot {
        revision: 1,
        sources: vec![instruction_source(10), instruction_source(20)],
        content_digest: "b".repeat(64),
    };
    snapshot.validate().unwrap();

    snapshot.sources.swap(0, 1);
    assert_eq!(
        snapshot.validate().unwrap_err().code,
        ErrorCode::InvalidProject
    );
}
