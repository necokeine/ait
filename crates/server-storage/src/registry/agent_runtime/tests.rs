use std::collections::BTreeMap;

use server_domain::agent_runtime::{AgentRuntimeStatus, PersistedAgentRuntimeRecord};
use server_ports::agent_runtime::{AgentRuntimeRegistry, AgentRuntimeRegistryError};

use super::FileBackedAgentRuntimeRegistry;

fn record(id: &str) -> PersistedAgentRuntimeRecord {
    PersistedAgentRuntimeRecord {
        id: id.to_owned(),
        provider: "codex".to_owned(),
        cwd: "/tmp/project".to_owned(),
        workspace_id: Some("wks_0123456789abcdef".to_owned()),
        created_at: "2026-09-20T10:00:00.000Z".to_owned(),
        updated_at: "2026-09-20T10:01:00.000Z".to_owned(),
        last_activity_at: None,
        last_user_message_at: None,
        title: None,
        labels: BTreeMap::new(),
        last_status: AgentRuntimeStatus::Closed,
        last_mode_id: None,
        config: None,
        runtime_info: None,
        features: Vec::new(),
        persistence: None,
        last_error: None,
        requires_attention: false,
        attention_reason: None,
        attention_timestamp: None,
        internal: false,
        archived_at: None,
        owner: None,
    }
}

#[test]
fn persists_updates_and_removals_across_reopen() {
    let temp = tempfile::tempdir().expect("temporary directory should exist");
    let path = temp.path().join("agents.json");
    let registry = FileBackedAgentRuntimeRegistry::new(path.clone());

    registry
        .upsert(&record("agent-1"))
        .expect("insert should persist");
    registry
        .update("agent-1", &|current| {
            let mut next = current.clone();
            next.title = Some("Renamed".to_owned());
            next
        })
        .expect("update should persist");

    let reopened = FileBackedAgentRuntimeRegistry::new(path.clone());
    assert_eq!(
        reopened
            .get("agent-1")
            .expect("read should succeed")
            .expect("record should exist")
            .title
            .as_deref(),
        Some("Renamed")
    );
    assert!(reopened.remove("agent-1").expect("remove should persist"));
    assert!(
        !reopened
            .remove("agent-1")
            .expect("missing remove is idempotent")
    );
    assert!(
        FileBackedAgentRuntimeRegistry::new(path)
            .list()
            .expect("reopen should succeed")
            .is_empty()
    );
}

#[test]
fn rejects_invalid_new_records_and_invalid_existing_files() {
    let temp = tempfile::tempdir().expect("temporary directory should exist");
    let path = temp.path().join("agents.json");
    let registry = FileBackedAgentRuntimeRegistry::new(path.clone());
    let mut invalid = record("");
    invalid.provider = String::new();
    assert_eq!(
        registry.upsert(&invalid),
        Err(AgentRuntimeRegistryError::InvalidRecord)
    );

    std::fs::write(&path, b"not json").expect("fixture should be writable");
    assert_eq!(
        FileBackedAgentRuntimeRegistry::new(path).initialize(),
        Err(AgentRuntimeRegistryError::InvalidFile)
    );
}

#[test]
fn update_cannot_change_agent_identity() {
    let temp = tempfile::tempdir().expect("temporary directory should exist");
    let registry = FileBackedAgentRuntimeRegistry::new(temp.path().join("agents.json"));
    registry
        .upsert(&record("agent-1"))
        .expect("insert should persist");

    assert_eq!(
        registry.update("agent-1", &|current| {
            let mut next = current.clone();
            next.id = "agent-2".to_owned();
            next
        }),
        Err(AgentRuntimeRegistryError::InvalidRecord)
    );
    assert!(
        registry
            .get("agent-1")
            .expect("read should succeed")
            .is_some()
    );
}
