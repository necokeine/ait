use super::*;

#[test]
fn stale_pointer_or_version_never_moves_the_reference() {
    let root = MessageId::from_u128(1);
    let child = MessageId::from_u128(2);
    let mut reference = SessionReference::new(root, AgentId::new("agent"));
    let original = reference.clone();
    assert!(reference.advance(root, 0, Some(root), child).is_err());
    assert_eq!(reference, original);
    assert!(reference.advance(root, 1, None, child).is_err());
    assert_eq!(reference, original);
    reference.advance(root, 1, Some(root), child).unwrap();
    assert_eq!(reference.head(), child);
    assert_eq!(reference.version(), 2);
}

#[test]
fn busy_binding_and_late_release_preserve_the_new_owner() {
    let mut reference = SessionReference::new(MessageId::from_u128(1), AgentId::new("agent"));
    reference.acquire(RunId::new("old")).unwrap();
    assert_eq!(
        reference.bind(AgentId::new("other")).unwrap_err().code,
        ErrorCode::SessionBusy
    );
    reference.release(&RunId::new("old"));
    reference.acquire(RunId::new("new")).unwrap();
    let expected = reference.clone();
    reference.release(&RunId::new("old"));
    assert_eq!(reference, expected);
    let encoded = serde_json::to_value(&reference).unwrap();
    assert_eq!(
        serde_json::from_value::<SessionReference>(encoded).unwrap(),
        reference
    );
}
