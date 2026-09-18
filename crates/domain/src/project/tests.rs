use std::path::PathBuf;

use super::*;

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
}
