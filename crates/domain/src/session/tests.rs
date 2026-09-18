use std::path::PathBuf;

use super::*;

#[test]
fn archived_session_cannot_retain_an_active_run() {
    let mut session = Session::new(
        SessionId::new("session-1"),
        ProjectId::new("project-1"),
        PathBuf::from("/project/.ait/session-1"),
        "main",
        MessageId::from_u128(1),
        AgentId::new("agent-1"),
        TimestampMs(1),
    );
    session.status = SessionStatus::Archived;
    session.active_run_id = Some(RunId::new("run-1"));

    assert_eq!(
        session.validate().unwrap_err().code,
        ErrorCode::InvalidSession
    );
}

#[test]
fn session_round_trip_keeps_snake_case_status() {
    let session = Session::new(
        SessionId::new("session-1"),
        ProjectId::new("project-1"),
        PathBuf::from("/project/.ait/session-1"),
        "main",
        MessageId::from_u128(1),
        AgentId::new("agent-1"),
        TimestampMs(1),
    );
    let encoded = serde_json::to_string(&session).unwrap();
    assert!(encoded.contains("\"status\":\"active\""));
    assert!(encoded.contains("\"agent_id\":\"agent-1\""));
    assert!(!encoded.contains("default_agent_id"));
    assert_eq!(serde_json::from_str::<Session>(&encoded).unwrap(), session);

    let mut missing_agent = session;
    missing_agent.agent_id = AgentId::new("");
    assert_eq!(
        missing_agent.validate().unwrap_err().code,
        ErrorCode::InvalidSession
    );
}

#[test]
fn session_name_may_be_empty_until_a_member_or_title_generator_names_it() {
    let session = Session::new(
        SessionId::new("session-1"),
        ProjectId::new("project-1"),
        PathBuf::from("/project/.ait/session-1"),
        "",
        MessageId::from_u128(1),
        AgentId::new("agent-1"),
        TimestampMs(1),
    );

    session.validate().unwrap();
    assert!(session.name.is_empty());
    assert!(session.description.is_empty());
}

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
