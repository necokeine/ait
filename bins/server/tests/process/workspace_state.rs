use std::collections::BTreeMap;

use serde_json::json;
use server_domain::agent_runtime::{
    AgentAttentionReason, AgentRuntimeStatus, PersistedAgentRuntimeRecord,
};
use server_domain::registry::{
    PersistedProjectKind, PersistedProjectRecord, PersistedWorkspaceKind, PersistedWorkspaceRecord,
};

use super::transport::{connect, receive, request};
use super::{ready, start, terminate};

const PARENT_LABEL: &str = "paseo.parent-agent-id";

#[tokio::test]
async fn binary_serves_workspace_attention_and_recovery_methods() {
    let root = tempfile::tempdir().expect("temporary directory");
    let state = root.path().join("server");
    let active_cwd = root.path().join("active");
    let archived_cwd = root.path().join("archived");
    std::fs::create_dir_all(&active_cwd).expect("active directory");
    std::fs::create_dir_all(&archived_cwd).expect("archived directory");
    seed(&state, &active_cwd, &archived_cwd);
    let log = root.path().join("server.log");
    let mut process = start(&state, &log);
    let address = ready(&mut process, &log).await;
    let mut client = connect(&address, server_protocol::workspace_state::CAPABILITIES).await;

    let cleared = request(
        &mut client,
        "workspace.clear_attention.request",
        json!({"workspaceId":["wks-active","missing"]}),
    )
    .await;
    assert_eq!(cleared["result"]["clearedAgentIds"], json!(["attention"]));
    assert_eq!(cleared["result"]["success"], false);
    assert_eq!(cleared["result"]["results"][0]["success"], true);
    assert_eq!(
        cleared["result"]["results"][1]["error"],
        "Workspace not found: missing"
    );

    let marked = request(
        &mut client,
        "workspace.mark_unread.request",
        json!({"workspaceId":"wks-active"}),
    )
    .await;
    assert_eq!(marked["result"]["markedAgentId"], "root");
    assert_eq!(marked["result"]["success"], true);

    let inspected = request(
        &mut client,
        "workspace.recovery.inspect.request",
        json!({"workspaceId":"wks-archived"}),
    )
    .await;
    assert_eq!(inspected["result"]["state"]["kind"], "recoverable");
    assert_eq!(inspected["result"]["state"]["action"], "unarchive");

    let restored = request(
        &mut client,
        "workspace.recovery.restore.request",
        json!({"workspaceId":"wks-archived"}),
    )
    .await;
    assert_eq!(restored["result"]["accepted"], true);
    let event = receive(&mut client).await;
    assert_eq!(event["type"], "event");
    assert_eq!(event["method"], "workspace.update");
    assert_eq!(event["params"]["workspace"]["id"], "wks-archived");

    let no_longer_archived = request(
        &mut client,
        "workspace.recovery.inspect.request",
        json!({"workspaceId":"wks-archived"}),
    )
    .await;
    assert_eq!(
        no_longer_archived["result"]["state"]["reason"],
        "workspace_not_archived"
    );

    terminate(&mut process).await;
    assert_persisted(&state);
}

fn seed(state: &std::path::Path, active_cwd: &std::path::Path, archived_cwd: &std::path::Path) {
    let projects = state.join("projects");
    std::fs::create_dir_all(&projects).expect("project registry directory");
    std::fs::write(
        projects.join("projects.json"),
        serde_json::to_vec_pretty(&[
            project("prj-active", active_cwd, false),
            project("prj-archived", archived_cwd, true),
        ])
        .expect("projects encode"),
    )
    .expect("projects persist");
    std::fs::write(
        projects.join("workspaces.json"),
        serde_json::to_vec_pretty(&[
            workspace("wks-active", "prj-active", active_cwd, false),
            workspace("wks-archived", "prj-archived", archived_cwd, true),
        ])
        .expect("workspaces encode"),
    )
    .expect("workspaces persist");

    let agents = state.join("agents");
    std::fs::create_dir_all(&agents).expect("Agent registry directory");
    let root = agent("root", false, None, "2026-09-22T10:00:00.000Z");
    let attention = agent(
        "attention",
        true,
        Some(AgentAttentionReason::Finished),
        "2026-09-22T12:00:00.000Z",
    );
    let permission = agent(
        "permission",
        true,
        Some(AgentAttentionReason::Permission),
        "2026-09-22T13:00:00.000Z",
    );
    std::fs::write(
        agents.join("agents.json"),
        serde_json::to_vec_pretty(&[root, attention, permission]).expect("Agents encode"),
    )
    .expect("Agents persist");
}

fn project(id: &str, root: &std::path::Path, archived: bool) -> PersistedProjectRecord {
    PersistedProjectRecord {
        project_id: id.to_owned(),
        root_path: root.to_str().expect("UTF-8 path").to_owned(),
        kind: PersistedProjectKind::NonGit,
        display_name: id.to_owned(),
        project_key: Some(id.to_owned()),
        custom_name: None,
        custom_icon_revision: None,
        created_at: "2026-09-22T09:00:00.000Z".to_owned(),
        updated_at: "2026-09-22T10:00:00.000Z".to_owned(),
        archived_at: archived.then(|| "2026-09-22T11:00:00.000Z".to_owned()),
    }
}

fn workspace(
    id: &str,
    project_id: &str,
    cwd: &std::path::Path,
    archived: bool,
) -> PersistedWorkspaceRecord {
    PersistedWorkspaceRecord {
        workspace_id: id.to_owned(),
        project_id: project_id.to_owned(),
        cwd: cwd.to_str().expect("UTF-8 path").to_owned(),
        kind: PersistedWorkspaceKind::Directory,
        display_name: id.to_owned(),
        title: None,
        branch: None,
        worktree_root: None,
        base_branch: None,
        is_paseo_owned_worktree: false,
        main_repo_root: None,
        created_at: "2026-09-22T09:00:00.000Z".to_owned(),
        updated_at: "2026-09-22T10:00:00.000Z".to_owned(),
        archived_at: archived.then(|| "2026-09-22T11:00:00.000Z".to_owned()),
        auto_archived_change_request_url: None,
        pinned_at: None,
        labels: None,
        untrusted_source: None,
    }
}

fn agent(
    id: &str,
    requires_attention: bool,
    attention_reason: Option<AgentAttentionReason>,
    updated_at: &str,
) -> PersistedAgentRuntimeRecord {
    PersistedAgentRuntimeRecord {
        id: id.to_owned(),
        provider: "codex".to_owned(),
        cwd: "/repo".to_owned(),
        workspace_id: Some("wks-active".to_owned()),
        created_at: "2026-09-22T09:00:00.000Z".to_owned(),
        updated_at: updated_at.to_owned(),
        last_activity_at: None,
        last_user_message_at: None,
        title: Some(id.to_owned()),
        labels: if id == "root" {
            BTreeMap::new()
        } else {
            BTreeMap::from([(PARENT_LABEL.to_owned(), "root".to_owned())])
        },
        last_status: AgentRuntimeStatus::Closed,
        last_mode_id: None,
        config: None,
        runtime_info: None,
        features: Vec::new(),
        persistence: None,
        last_error: None,
        requires_attention,
        attention_reason,
        attention_timestamp: requires_attention.then(|| updated_at.to_owned()),
        internal: false,
        archived_at: None,
        owner: None,
    }
}

fn assert_persisted(state: &std::path::Path) {
    let agents: Vec<PersistedAgentRuntimeRecord> = serde_json::from_slice(
        &std::fs::read(state.join("agents/agents.json")).expect("Agent registry"),
    )
    .expect("Agent records");
    assert!(
        agents
            .iter()
            .find(|agent| agent.id == "root")
            .expect("root")
            .requires_attention
    );
    assert!(
        !agents
            .iter()
            .find(|agent| agent.id == "attention")
            .expect("attention")
            .requires_attention
    );
    assert!(
        agents
            .iter()
            .find(|agent| agent.id == "permission")
            .expect("permission")
            .requires_attention
    );
    let workspaces: Vec<PersistedWorkspaceRecord> = serde_json::from_slice(
        &std::fs::read(state.join("projects/workspaces.json")).expect("Workspace registry"),
    )
    .expect("Workspace records");
    assert!(
        workspaces
            .iter()
            .find(|workspace| workspace.workspace_id == "wks-archived")
            .expect("archived Workspace")
            .archived_at
            .is_none()
    );
}
