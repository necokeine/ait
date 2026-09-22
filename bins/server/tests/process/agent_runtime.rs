use std::collections::BTreeMap;

use serde_json::json;
use server_domain::agent_runtime::{
    AgentAttentionReason, AgentRuntimeStatus, PersistedAgentRuntimeRecord,
};
use server_domain::registry::{
    PersistedProjectKind, PersistedProjectRecord, PersistedWorkspaceKind, PersistedWorkspaceRecord,
};

use super::transport::{Socket, connect, request};
use super::{ready, start, terminate};

const PARENT_LABEL: &str = "paseo.parent-agent-id";

#[tokio::test]
async fn binary_serves_agent_runtime_directory_and_metadata_lifecycle() {
    let root = tempfile::tempdir().expect("temporary directory should exist");
    let state = root.path().join("server");
    seed(&state);
    let log = root.path().join("server.log");
    let mut process = start(&state, &log);
    let address = ready(&mut process, &log).await;
    let mut client = connect(&address, server_protocol::agent_lifecycle::CAPABILITIES).await;

    assert_directory_reads(&mut client).await;
    assert_metadata_mutations(&mut client).await;
    assert_archive_close_delete(&mut client).await;

    terminate(&mut process).await;
    assert_persisted_state(&state);
}

async fn assert_directory_reads(client: &mut Socket) {
    let listed = request(
        client,
        "agent.list.request",
        json!({"sort":[{"key":"title","direction":"asc"}]}),
    )
    .await;
    assert_eq!(
        listed["result"]["entries"].as_array().map(Vec::len),
        Some(4)
    );
    assert_eq!(
        listed["result"]["entries"][0]["project"]["projectKey"],
        "project-key"
    );
    assert_eq!(
        listed["result"]["entries"][0]["agent"]["providerUnavailable"],
        true
    );

    let history = request(
        client,
        "agent.history.get.request",
        json!({"search":"main"}),
    )
    .await;
    assert_eq!(
        history["result"]["entries"].as_array().map(Vec::len),
        Some(5)
    );
    let fetched = request(client, "agent.get.request", json!({"agentId":"agent-par"})).await;
    assert_eq!(fetched["result"]["agent"]["id"], "agent-parent");
}

async fn assert_metadata_mutations(client: &mut Socket) {
    assert_eq!(
        request(
            client,
            "agent.update.request",
            json!({"agentId":"agent-parent","name":"  Renamed  ","labels":{"team":"new"}})
        )
        .await["result"]["accepted"],
        true
    );
    let cleared = request(
        client,
        "agent.attention.clear.request",
        json!({"agentId":"agent-parent"}),
    )
    .await;
    assert_eq!(cleared["result"]["agents"][0]["requiresAttention"], false);
    assert_eq!(
        request(
            client,
            "agent.detach.request",
            json!({"agentId":"agent-detach"})
        )
        .await["result"]["accepted"],
        true
    );
}

async fn assert_archive_close_delete(client: &mut Socket) {
    let archived = request(
        client,
        "agent.archive.request",
        json!({"agentId":"agent-parent"}),
    )
    .await;
    assert!(archived["result"]["archivedAt"].is_string());
    assert!(
        request(
            client,
            "agent.get.request",
            json!({"agentId":"agent-child"})
        )
        .await["result"]["agent"]["archivedAt"]
            .is_string()
    );
    assert_eq!(
        request(
            client,
            "agent.get.request",
            json!({"agentId":"agent-handoff"})
        )
        .await["result"]["agent"]["labels"][PARENT_LABEL],
        serde_json::Value::Null
    );

    assert_eq!(
        request(
            client,
            "agent.items.close.request",
            json!({"agentIds":["agent-handoff"],"terminalIds":[]})
        )
        .await["result"]["agents"][0]["agentId"],
        "agent-handoff"
    );
    assert_eq!(
        request(
            client,
            "agent.delete.request",
            json!({"agentId":"agent-child"})
        )
        .await["result"]["agentId"],
        "agent-child"
    );
    assert_eq!(
        request(client, "fetch_agents_request", json!({})).await["code"],
        "method_not_found"
    );
}

fn assert_persisted_state(state: &std::path::Path) {
    let records: Vec<PersistedAgentRuntimeRecord> = serde_json::from_slice(
        &std::fs::read(state.join("agents/agents.json")).expect("registry should persist"),
    )
    .expect("registry should decode");
    assert!(!records.iter().any(|record| record.id == "agent-child"));
    let parent = records
        .iter()
        .find(|record| record.id == "agent-parent")
        .expect("parent should remain archived");
    assert_eq!(parent.title.as_deref(), Some("Renamed"));
    assert!(!parent.requires_attention);
}

fn seed(state: &std::path::Path) {
    let projects = state.join("projects");
    std::fs::create_dir_all(&projects).expect("registry directory should exist");
    std::fs::write(
        projects.join("projects.json"),
        serde_json::to_vec_pretty(&[project()]).expect("project should encode"),
    )
    .expect("project registry should be writable");
    std::fs::write(
        projects.join("workspaces.json"),
        serde_json::to_vec_pretty(&[workspace("wks-main"), workspace("wks-other")])
            .expect("workspaces should encode"),
    )
    .expect("workspace registry should be writable");
    let agents = state.join("agents");
    std::fs::create_dir_all(&agents).expect("Agent registry directory should exist");
    let mut parent = agent("agent-parent", "wks-main", "Parent");
    parent.requires_attention = true;
    parent.attention_reason = Some(AgentAttentionReason::Finished);
    parent.attention_timestamp = Some("2026-09-20T12:00:00.000Z".to_owned());
    let mut child = agent("agent-child", "wks-main", "Child");
    child
        .labels
        .insert(PARENT_LABEL.to_owned(), "agent-parent".to_owned());
    let mut handoff = agent("agent-handoff", "wks-other", "Handoff");
    handoff
        .labels
        .insert(PARENT_LABEL.to_owned(), "agent-parent".to_owned());
    let mut detached = agent("agent-detach", "wks-main", "Detach");
    detached
        .labels
        .insert(PARENT_LABEL.to_owned(), "agent-parent".to_owned());
    detached
        .labels
        .insert("paseo.open-agent-tab.desktop".to_owned(), "true".to_owned());
    let mut archived = agent("agent-archived", "wks-main", "Archived");
    archived.archived_at = Some("2026-09-19T00:00:00.000Z".to_owned());
    std::fs::write(
        agents.join("agents.json"),
        serde_json::to_vec_pretty(&[parent, child, handoff, detached, archived])
            .expect("Agents should encode"),
    )
    .expect("Agent registry should be writable");
}

fn agent(id: &str, workspace_id: &str, title: &str) -> PersistedAgentRuntimeRecord {
    PersistedAgentRuntimeRecord {
        id: id.to_owned(),
        provider: "codex".to_owned(),
        cwd: "/repo".to_owned(),
        workspace_id: Some(workspace_id.to_owned()),
        created_at: "2026-09-20T10:00:00.000Z".to_owned(),
        updated_at: "2026-09-20T11:00:00.000Z".to_owned(),
        last_activity_at: None,
        last_user_message_at: None,
        title: Some(title.to_owned()),
        labels: BTreeMap::from([("team".to_owned(), "server".to_owned())]),
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

fn project() -> PersistedProjectRecord {
    PersistedProjectRecord {
        project_id: "prj-main".to_owned(),
        root_path: "/repo".to_owned(),
        kind: PersistedProjectKind::Git,
        display_name: "Project".to_owned(),
        project_key: Some("project-key".to_owned()),
        custom_name: None,
        custom_icon_revision: None,
        created_at: "2026-09-20T00:00:00.000Z".to_owned(),
        updated_at: "2026-09-20T00:00:00.000Z".to_owned(),
        archived_at: None,
    }
}

fn workspace(id: &str) -> PersistedWorkspaceRecord {
    PersistedWorkspaceRecord {
        workspace_id: id.to_owned(),
        project_id: "prj-main".to_owned(),
        cwd: "/repo".to_owned(),
        kind: PersistedWorkspaceKind::LocalCheckout,
        display_name: "Main".to_owned(),
        title: None,
        branch: Some("main".to_owned()),
        worktree_root: Some("/repo".to_owned()),
        base_branch: None,
        is_paseo_owned_worktree: false,
        main_repo_root: None,
        created_at: "2026-09-20T00:00:00.000Z".to_owned(),
        updated_at: "2026-09-20T00:00:00.000Z".to_owned(),
        archived_at: None,
        auto_archived_change_request_url: None,
        pinned_at: None,
        labels: None,
        untrusted_source: None,
    }
}
