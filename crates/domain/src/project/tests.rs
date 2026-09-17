use std::path::PathBuf;

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

#[test]
fn message_id_is_a_uuid_with_stable_serde() {
    let id = MessageId::from_u128(1);
    let encoded = serde_json::to_string(&id).unwrap();
    assert_eq!(encoded, "\"00000000-0000-0000-0000-000000000001\"");
    assert_eq!(serde_json::from_str::<MessageId>(&encoded).unwrap(), id);
    assert!(MessageId::parse("message-1").is_err());
}

#[test]
fn empty_project_description_is_serialized_and_the_deserialization_default() {
    let project = Project {
        id: ProjectId::new("project-1"),
        name: "Project".into(),
        description: String::new(),
        workdir: PathBuf::from("/project"),
        git_initialized_by_manager: false,
        repo_url: None,
        base_commit: GitCommit::parse("b".repeat(40)).unwrap(),
        default_agent_id: None,
        instruction_revision: 1,
        instruction_digest: "a".repeat(64),
        metadata: DomainMetadata::default(),
        status: ProjectStatus::Active,
        created_at: TimestampMs(1),
        updated_at: TimestampMs(1),
    };
    let encoded = serde_json::to_string(&project).unwrap();
    assert!(encoded.contains("\"description\":\"\""));
    assert_eq!(serde_json::from_str::<Project>(&encoded).unwrap(), project);

    let mut without_description = serde_json::to_value(&project).unwrap();
    without_description
        .as_object_mut()
        .unwrap()
        .remove("description");
    assert_eq!(
        serde_json::from_value::<Project>(without_description)
            .unwrap()
            .description,
        ""
    );
}

#[test]
fn project_requires_base_commit_and_nonempty_repo_url_when_present() {
    let mut project = Project {
        id: ProjectId::new("project-1"),
        name: "Project".into(),
        description: String::new(),
        workdir: PathBuf::from("/project"),
        git_initialized_by_manager: false,
        repo_url: Some("git@github.com:member/fork.git".into()),
        base_commit: GitCommit::parse("b".repeat(40)).unwrap(),
        default_agent_id: None,
        instruction_revision: 1,
        instruction_digest: "a".repeat(64),
        metadata: DomainMetadata::default(),
        status: ProjectStatus::Active,
        created_at: TimestampMs(1),
        updated_at: TimestampMs(1),
    };
    project.validate().unwrap();

    project.repo_url = Some("  ".into());
    assert_eq!(
        project.validate().unwrap_err().code,
        ErrorCode::InvalidProject
    );

    project.repo_url = None;
    assert!(serde_json::from_str::<GitCommit>("\"short\"").is_err());
}
