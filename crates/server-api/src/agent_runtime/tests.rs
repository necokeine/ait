use std::collections::BTreeMap;

use serde_json::json;
use server_domain::agent_runtime::{AgentRuntimeStatus, PersistedAgentRuntimeRecord};
use server_protocol::agent_lifecycle::{AgentHistoryRequest, AgentListRequest};

use super::{project, query, snapshot};
use server_application::agent_runtime::AgentPlacement;

#[test]
fn history_defaults_to_archived_while_active_list_does_not() {
    let list: AgentListRequest = serde_json::from_value(json!({})).expect("list request");
    let history: AgentHistoryRequest = serde_json::from_value(json!({})).expect("history request");

    let list_query =
        query(list.filter, list.sort, list.page, false, false, None).expect("list query");
    let history_query = query(
        history.filter,
        history.sort,
        history.page,
        false,
        true,
        history.search,
    )
    .expect("history query");

    assert!(!list_query.include_archived);
    assert!(history_query.include_archived);
    assert_eq!(list_query.limit, 200);

    let empty_filters: AgentListRequest = serde_json::from_value(json!({
        "filter": {"projectKeys":["  "],"statuses":[]}
    }))
    .expect("empty filters");
    let empty_query = query(
        empty_filters.filter,
        empty_filters.sort,
        empty_filters.page,
        false,
        false,
        None,
    )
    .expect("empty filters should be no-ops");
    assert!(empty_query.project_keys.is_none());
    assert!(empty_query.statuses.is_none());
}

#[test]
fn stored_projection_marks_provider_unavailable_and_hides_resume_handle() {
    let mut record = record();
    record.updated_at = "2026-09-20T09:00:00-02:00".to_owned();
    record.last_activity_at = Some("2026-09-20T12:00:00+00:00".to_owned());
    record.features = vec![json!({"id":"stored-only"})];
    record.last_error = Some("old provider error".to_owned());
    record.persistence = Some(server_domain::agent_runtime::AgentPersistenceHandle {
        provider: "codex".to_owned(),
        session_id: "thread-1".to_owned(),
        native_handle: None,
        metadata: None,
    });

    let projected = snapshot(&record);
    assert!(projected.provider_unavailable);
    assert!(projected.persistence.is_none());
    assert_eq!(
        projected.capabilities.get("supportsSessionPersistence"),
        Some(&true)
    );
    assert_eq!(
        projected.status,
        server_protocol::agent_lifecycle::AgentStatus::Closed
    );
    assert_eq!(projected.updated_at, "2026-09-20T12:00:00.000Z");
    assert!(projected.features.is_empty());
    assert!(projected.last_error.is_none());
}

#[test]
fn placement_projection_preserves_managed_worktree_identity() {
    let projected = project(&AgentPlacement {
        active: true,
        project_key: "github:owner/repo".to_owned(),
        project_name: "Repo".to_owned(),
        workspace_name: "Feature".to_owned(),
        cwd: "/worktrees/feature/subdir".to_owned(),
        is_git: true,
        current_branch: Some("feature".to_owned()),
        worktree_root: Some("/worktrees/feature".to_owned()),
        is_paseo_owned_worktree: true,
        main_repo_root: Some("/repo".to_owned()),
    });

    assert_eq!(projected.workspace_name, Some(Some("Feature".to_owned())));
    assert!(projected.checkout.is_paseo_owned_worktree);
    assert_eq!(projected.checkout.main_repo_root.as_deref(), Some("/repo"));
}

fn record() -> PersistedAgentRuntimeRecord {
    PersistedAgentRuntimeRecord {
        id: "agent-1".to_owned(),
        provider: "codex".to_owned(),
        cwd: "/repo".to_owned(),
        workspace_id: Some("wks-1".to_owned()),
        created_at: "2026-09-20T00:00:00.000Z".to_owned(),
        updated_at: "2026-09-20T00:01:00.000Z".to_owned(),
        last_activity_at: None,
        last_user_message_at: None,
        title: Some("Agent".to_owned()),
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
