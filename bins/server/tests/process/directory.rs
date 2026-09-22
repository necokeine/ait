use std::path::Path;

use serde_json::json;
use server_domain::registry::{
    PersistedProjectKind, PersistedProjectRecord, PersistedWorkspaceKind, PersistedWorkspaceRecord,
};

use super::transport::{Socket, connect, request};
use super::{ready, start, terminate};

#[tokio::test]
async fn binary_serves_canonical_project_workspace_directory_methods() {
    let root = tempfile::tempdir().unwrap();
    let state = root.path().join("server");
    let registry = state.join("projects");
    std::fs::create_dir_all(&registry).unwrap();
    std::fs::write(
        registry.join("projects.json"),
        serde_json::to_vec_pretty(&[project()]).unwrap(),
    )
    .unwrap();
    std::fs::write(
        registry.join("workspaces.json"),
        serde_json::to_vec_pretty(&[workspace()]).unwrap(),
    )
    .unwrap();
    let existing = root.path().join("existing");
    let parent = root.path().join("parent");
    std::fs::create_dir(&existing).unwrap();
    std::fs::create_dir(&parent).unwrap();
    let log = root.path().join("server.log");
    let mut process = start(&state, &log);
    let address = ready(&mut process, &log).await;
    let capabilities = server_protocol::directory::CAPABILITIES
        .iter()
        .chain(server_protocol::project_config::CAPABILITIES)
        .chain(server_protocol::project_icon::CAPABILITIES)
        .copied()
        .collect::<Vec<_>>();
    let mut client = connect(&address, &capabilities).await;

    assert_seeded_directory(&mut client).await;
    let added_project_id = add_project(&mut client, &existing).await;
    assert_project_config(&mut client, &existing).await;
    assert_project_icon(&mut client, &added_project_id).await;
    let opened_id = assert_workspace_creation(&mut client, &existing, &added_project_id).await;
    assert_directory_creation_and_errors(&mut client, &parent, &existing).await;
    assert_metadata_updates(&mut client).await;
    assert_legacy_names_are_rejected(&mut client, &existing).await;
    assert_archiving_and_restore(&mut client, &existing, &opened_id).await;
    terminate(&mut process).await;

    assert_persisted_changes(&registry);
}

async fn assert_seeded_directory(client: &mut Socket) {
    let projects = request(client, "project.list.request", json!({})).await;
    assert_eq!(projects["result"]["projects"][0]["projectId"], "prj_a");
    let workspaces = request(
        client,
        "workspace.list.request",
        json!({"filter":{"query":"ALPHA"},"page":{"limit":20}}),
    )
    .await;
    assert_eq!(workspaces["result"]["entries"][0]["id"], "wks_a");
}

async fn add_project(client: &mut Socket, existing: &Path) -> String {
    let added = request(client, "project.add.request", json!({"cwd":existing})).await;
    assert_eq!(
        added["result"]["project"]["projectRootPath"],
        existing.to_str().unwrap()
    );
    added["result"]["project"]["projectId"]
        .as_str()
        .unwrap()
        .to_owned()
}

async fn assert_project_config(client: &mut Socket, existing: &Path) {
    let config = request(
        client,
        "project.config.read.request",
        json!({"repoRoot":existing}),
    )
    .await;
    assert_eq!(config["result"]["ok"], true);
    assert!(config["result"]["config"].is_null());
    let written_config = request(
        client,
        "project.config.write.request",
        json!({
            "repoRoot":existing,
            "config":{"worktree":{"setup":"npm ci"},"future":true},
            "expectedRevision":null
        }),
    )
    .await;
    assert_eq!(written_config["result"]["ok"], true);
    assert!(written_config["result"]["revision"]["mtimeMs"].is_number());
    assert_eq!(
        request(
            client,
            "project.config.write.request",
            json!({"repoRoot":existing,"config":{},"expectedRevision":null})
        )
        .await["result"]["error"]["code"],
        "stale_project_config"
    );
    assert_eq!(
        request(
            client,
            "project.config.read.request",
            json!({"repoRoot":existing})
        )
        .await["result"]["config"]["worktree"]["setup"],
        "npm ci"
    );
}

async fn assert_project_icon(client: &mut Socket, added_project_id: &str) {
    let icon_data = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAIAAAAAAAA=";
    assert_eq!(
        request(
            client,
            "project.icon.set.request",
            json!({
                "projectId":added_project_id,
                "source":{"type":"upload","data":icon_data}
            })
        )
        .await["result"]["accepted"],
        true
    );
    let icon = request(
        client,
        "project.icon.get.request",
        json!({"projectId":added_project_id}),
    )
    .await;
    assert_eq!(icon["result"]["icon"]["data"], icon_data);
    assert_eq!(icon["result"]["icon"]["mimeType"], "image/png");
    assert_eq!(
        request(
            client,
            "project.icon.set.request",
            json!({
                "projectId":added_project_id,
                "source":{"type":"url","url":"http://127.0.0.1/private"}
            })
        )
        .await["code"],
        "invalid_message"
    );
    assert_eq!(
        request(
            client,
            "project.icon.set.request",
            json!({"projectId":added_project_id,"source":{"type":"automatic"}})
        )
        .await["result"]["accepted"],
        true
    );
    assert!(
        request(
            client,
            "project.icon.get.request",
            json!({"projectId":added_project_id})
        )
        .await["result"]["icon"]
            .is_null()
    );
}

async fn assert_workspace_creation(
    client: &mut Socket,
    existing: &Path,
    added_project_id: &str,
) -> String {
    let opened = request(client, "workspace.open.request", json!({"cwd":existing})).await;
    let opened_id = opened["result"]["workspace"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        request(client, "workspace.open.request", json!({"cwd":existing})).await["result"]["workspace"]
            ["id"],
        opened_id
    );
    let created = request(
        client,
        "workspace.create.request",
        json!({
            "workspaceId":"wks_0123456789abcdef",
            "title":"  Fresh  ",
            "source":{"kind":"directory","path":existing,"projectId":added_project_id}
        }),
    )
    .await;
    assert_eq!(created["result"]["workspace"]["id"], "wks_0123456789abcdef");
    assert_eq!(created["result"]["workspace"]["title"], "Fresh");
    opened_id
}

async fn assert_directory_creation_and_errors(client: &mut Socket, parent: &Path, existing: &Path) {
    let created_directory = request(
        client,
        "project.create_directory.request",
        json!({"parentPath":parent,"name":"new-project"}),
    )
    .await;
    assert!(parent.join("new-project").is_dir());
    assert_eq!(
        created_directory["result"]["directoryPath"],
        parent.join("new-project").to_str().unwrap()
    );
    assert_eq!(
        request(
            client,
            "project.create_directory.request",
            json!({"parentPath":parent,"name":"new-project"})
        )
        .await["result"]["errorCode"],
        "directory_exists"
    );
    assert_eq!(
        request(
            client,
            "workspace.create.request",
            json!({"source":{"kind":"directory","path":existing,"projectId":"prj_missing"}})
        )
        .await["result"]["errorCode"],
        "unknown_project"
    );
    assert_eq!(
        request(
            client,
            "workspace.create.request",
            json!({"source":{"kind":"worktree","cwd":existing}})
        )
        .await["code"],
        "unsupported_capability"
    );
}

async fn assert_metadata_updates(client: &mut Socket) {
    let renamed = request(
        client,
        "project.rename.request",
        json!({"projectId":"prj_a","customName":"  Alpha UI  "}),
    )
    .await;
    assert_eq!(renamed["result"]["customName"], "Alpha UI");
    let pinned = request(
        client,
        "workspace.pin.set.request",
        json!({"workspaceId":"wks_a","pinned":true}),
    )
    .await;
    assert!(pinned["result"]["pinnedAt"].is_string());
    let titled = request(
        client,
        "workspace.title.set.request",
        json!({"workspaceId":"wks_a","title":"  Review  "}),
    )
    .await;
    assert_eq!(titled["result"]["title"], "Review");

    assert_eq!(
        request(client, "workspace.list.request", json!({"subscribe":{}})).await["code"],
        "unsupported_capability"
    );
}

async fn assert_legacy_names_are_rejected(client: &mut Socket, existing: &Path) {
    assert_eq!(
        request(
            client,
            "read_project_config_request",
            json!({"repoRoot":"/tmp/alpha"})
        )
        .await["code"],
        "method_not_found"
    );
    assert_eq!(
        request(client, "project_icon_request", json!({"cwd":existing})).await["code"],
        "method_not_found"
    );
    assert_eq!(
        request(
            client,
            "write_project_config_request",
            json!({"repoRoot":existing,"config":{},"expectedRevision":null})
        )
        .await["code"],
        "method_not_found"
    );
}

async fn assert_archiving_and_restore(client: &mut Socket, existing: &Path, opened_id: &str) {
    let archived = request(
        client,
        "workspace.archive.request",
        json!({"workspaceId":"wks_a"}),
    )
    .await;
    assert!(archived["result"]["archivedAt"].is_string());
    assert!(
        request(client, "workspace.list.request", json!({})).await["result"]["entries"]
            .as_array()
            .unwrap()
            .as_slice()
            .iter()
            .all(|entry| entry["id"] != "wks_a")
    );

    let archived_opened = request(
        client,
        "workspace.archive.request",
        json!({"workspaceId":opened_id}),
    )
    .await;
    assert!(archived_opened["result"]["archivedAt"].is_string());
    let archived_fresh = request(
        client,
        "workspace.archive.request",
        json!({"workspaceId":"wks_0123456789abcdef"}),
    )
    .await;
    assert!(archived_fresh["result"]["archivedAt"].is_string());
    assert_eq!(
        request(client, "workspace.open.request", json!({"cwd":existing})).await["result"]["workspace"]
            ["id"],
        opened_id
    );
}

fn assert_persisted_changes(registry: &Path) {
    let persisted_projects: Vec<PersistedProjectRecord> =
        serde_json::from_slice(&std::fs::read(registry.join("projects.json")).unwrap()).unwrap();
    let persisted_workspaces: Vec<PersistedWorkspaceRecord> =
        serde_json::from_slice(&std::fs::read(registry.join("workspaces.json")).unwrap()).unwrap();
    let original_project = persisted_projects
        .iter()
        .find(|project| project.project_id == "prj_a")
        .unwrap();
    let original_workspace = persisted_workspaces
        .iter()
        .find(|workspace| workspace.workspace_id == "wks_a")
        .unwrap();
    assert_eq!(original_project.custom_name.as_deref(), Some("Alpha UI"));
    assert_eq!(original_workspace.title.as_deref(), Some("Review"));
    assert!(original_workspace.archived_at.is_some());
    assert!(persisted_projects.len() >= 3);
    assert!(persisted_workspaces.len() >= 3);
}

fn project() -> PersistedProjectRecord {
    PersistedProjectRecord {
        project_id: "prj_a".to_owned(),
        root_path: "/tmp/alpha".to_owned(),
        kind: PersistedProjectKind::Git,
        display_name: "alpha".to_owned(),
        project_key: None,
        custom_name: None,
        custom_icon_revision: None,
        created_at: "2026-09-20T00:00:00.000Z".to_owned(),
        updated_at: "2026-09-20T00:00:00.000Z".to_owned(),
        archived_at: None,
    }
}

fn workspace() -> PersistedWorkspaceRecord {
    PersistedWorkspaceRecord {
        workspace_id: "wks_a".to_owned(),
        project_id: "prj_a".to_owned(),
        cwd: "/tmp/alpha".to_owned(),
        kind: PersistedWorkspaceKind::LocalCheckout,
        display_name: "main".to_owned(),
        title: None,
        branch: Some("main".to_owned()),
        worktree_root: Some("/tmp/alpha".to_owned()),
        base_branch: Some("main".to_owned()),
        is_paseo_owned_worktree: false,
        main_repo_root: Some("/tmp/alpha".to_owned()),
        created_at: "2026-09-20T00:00:00.000Z".to_owned(),
        updated_at: "2026-09-20T00:00:00.000Z".to_owned(),
        archived_at: None,
        auto_archived_change_request_url: None,
        pinned_at: None,
        labels: None,
        untrusted_source: None,
    }
}
