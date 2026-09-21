use super::*;

#[test]
fn identities_and_commit_ids_reject_invalid_values() {
    let project = ProjectId::generate();
    let message = MessageId::generate();
    let operation = OperationId::generate();
    assert_eq!(project.to_string().parse(), Ok(project));
    assert_eq!(message.to_string().parse(), Ok(message));
    assert_eq!(operation.to_string().parse(), Ok(operation));
    for invalid in ["", "not-an-id", "00000000-0000-0000-0000-000000000000"] {
        assert!(invalid.parse::<ProjectId>().is_err());
        assert!(invalid.parse::<MessageId>().is_err());
        assert!(invalid.parse::<OperationId>().is_err());
    }
    for length in [40, 64] {
        assert_eq!(
            "A".repeat(length).parse::<GitCommit>().unwrap().to_string(),
            "a".repeat(length)
        );
    }
    for invalid in ["a".repeat(39), "a".repeat(65), "z".repeat(40)] {
        assert!(invalid.parse::<GitCommit>().is_err());
    }
}

#[test]
fn project_creation_is_validated_and_snapshots_are_immutable_values() {
    let root = RootMessage::new(MessageId::generate(), "instructions".to_owned(), 42).unwrap();
    let id = ProjectId::generate();
    let head: GitCommit = "a".repeat(40).parse().unwrap();
    let project = Project::new(id, "repository".to_owned(), head.clone(), root.clone()).unwrap();
    assert_eq!(project.id(), id);
    assert_eq!(project.name(), "repository");
    assert_eq!(project.base_commit(), &head);
    assert_eq!(project.root(), &root);
    assert_eq!(root.text(), "instructions");
    assert_eq!(root.created_at(), 42);
    for name in [String::new(), "a".repeat(256), "bad\nname".to_owned()] {
        assert!(Project::new(id, name, head.clone(), root.clone()).is_err());
    }
    assert!(RootMessage::new(root.id(), "a".repeat(MAX_INSTRUCTION_BYTES + 1), 42).is_err());
    assert!(RootMessage::new(root.id(), String::new(), u64::MAX).is_err());
    assert!(OwnerEpoch::new(u64::MAX).is_err());
    assert_eq!(OwnerEpoch::new(7).unwrap().value(), 7);
}
