use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::json;
use server_filesystem::protocol::worktrees::CAPABILITIES;

use super::transport::{Socket, connect, receive, request};
use super::{ready, start, terminate};

#[tokio::test]
async fn binary_creates_lists_and_archives_canonical_worktrees() {
    let root = tempfile::tempdir().unwrap();
    let repository = root.path().join("repository");
    create_repository(&repository);
    let nested = repository.join("packages/app");
    std::fs::write(nested.join("paseo.json"), "{\"scripts\":{}}\n").unwrap();

    let state = root.path().join("server");
    let log = root.path().join("server.log");
    let mut process = start(&state, &log);
    let address = ready(&mut process, &log).await;
    let mut client = connect(&address, CAPABILITIES).await;

    assert_empty_list_and_required_location(&mut client, &repository).await;

    let created = request(
        &mut client,
        "workspace.worktree.create.request",
        json!({
            "cwd":nested,
            "worktreeSlug":"Feature Review!",
            "nameContext":"  Review   this change  ",
            "attachments":[],
            "action":"branch-off",
            "refName":"main"
        }),
    )
    .await;
    assert_eq!(created["type"], "response", "{created}");
    assert!(created["result"]["error"].is_null(), "{created}");
    assert!(created["result"]["setupTerminalId"].is_null());
    assert_eq!(
        created["result"]["workspace"]["title"],
        "Review this change"
    );
    assert_eq!(created["result"]["workspace"]["workspaceKind"], "worktree");
    assert_eq!(
        created["result"]["workspace"]["worktreeSlug"],
        "feature-review"
    );
    let workspace_id = created["result"]["workspace"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let workspace_directory = PathBuf::from(
        created["result"]["workspace"]["workspaceDirectory"]
            .as_str()
            .unwrap(),
    );
    assert_eq!(
        std::fs::read_to_string(workspace_directory.join("paseo.json")).unwrap(),
        "{\"scripts\":{}}\n"
    );
    assert_eq!(branch(&workspace_directory), "feature-review");

    let event = receive(&mut client).await;
    assert_eq!(event["type"], "event");
    assert_eq!(event["method"], "workspace.update");
    assert_eq!(event["params"]["kind"], "upsert");
    assert_eq!(event["params"]["workspace"]["id"], workspace_id);

    let listed = request(
        &mut client,
        "workspace.worktree.list.request",
        json!({"repoRoot":repository}),
    )
    .await;
    assert_eq!(listed["result"]["worktrees"].as_array().unwrap().len(), 1);
    assert_eq!(
        listed["result"]["worktrees"][0]["branchName"],
        "feature-review"
    );
    assert!(listed["result"]["worktrees"][0]["head"].is_string());
    let worktree_root = PathBuf::from(
        listed["result"]["worktrees"][0]["worktreePath"]
            .as_str()
            .unwrap(),
    );
    assert!(workspace_directory.starts_with(&worktree_root));

    assert_checkout_requires_target(&mut client, &repository).await;

    let archived = request(
        &mut client,
        "workspace.worktree.archive.request",
        json!({
            "worktreePath":workspace_directory,
            "workspaceId":workspace_id,
            "deleteWorktreeFromDisk":false
        }),
    )
    .await;
    assert_eq!(archived["result"]["success"], true, "{archived}");
    assert_eq!(archived["result"]["removedAgents"], json!([]));
    assert!(!worktree_root.exists());
    let after = request(
        &mut client,
        "workspace.worktree.list.request",
        json!({"cwd":repository}),
    )
    .await;
    assert_eq!(after["result"]["worktrees"], json!([]));

    assert_worktree_scope_rejects_external_path(&mut client, &repository).await;

    terminate(&mut process).await;
    assert_persisted_workspace(&state, &workspace_id);
}

fn assert_persisted_workspace(state: &Path, workspace_id: &str) {
    let workspaces: serde_json::Value =
        serde_json::from_slice(&std::fs::read(state.join("projects/workspaces.json")).unwrap())
            .unwrap();
    assert_eq!(workspaces[0]["workspaceId"], workspace_id);
    assert!(workspaces[0]["archivedAt"].is_string());
    assert_eq!(workspaces[0]["isPaseoOwnedWorktree"], true);
}

async fn assert_empty_list_and_required_location(client: &mut Socket, repository: &Path) {
    let empty = request(
        client,
        "workspace.worktree.list.request",
        json!({"cwd":repository}),
    )
    .await;
    assert_eq!(empty["result"]["worktrees"], json!([]));
    assert!(empty["result"]["error"].is_null());
    let missing_location = request(client, "workspace.worktree.list.request", json!({})).await;
    assert_eq!(missing_location["result"]["error"]["code"], "UNKNOWN");
}

async fn assert_checkout_requires_target(client: &mut Socket, repository: &Path) {
    let invalid_checkout = request(
        client,
        "workspace.worktree.create.request",
        json!({"cwd":repository,"action":"checkout"}),
    )
    .await;
    assert_eq!(
        invalid_checkout["result"]["errorCode"],
        "missing_checkout_target"
    );
}

async fn assert_worktree_scope_rejects_external_path(client: &mut Socket, repository: &Path) {
    let rejected = request(
        client,
        "workspace.worktree.archive.request",
        json!({"worktreePath":repository,"scope":"worktree"}),
    )
    .await;
    assert_eq!(rejected["result"]["success"], false);
    assert_eq!(rejected["result"]["error"]["code"], "NOT_ALLOWED");
}

fn create_repository(repository: &Path) {
    std::fs::create_dir_all(repository.join("packages/app")).unwrap();
    run(repository, &["init", "--quiet", "--initial-branch=main"]);
    std::fs::write(repository.join("README.md"), "baseline\n").unwrap();
    std::fs::write(repository.join("packages/app/tracked.txt"), "tracked\n").unwrap();
    run(repository, &["add", "."]);
    run(
        repository,
        &[
            "-c",
            "user.name=Server Test",
            "-c",
            "user.email=server@example.invalid",
            "commit",
            "--quiet",
            "-m",
            "baseline",
        ],
    );
}

fn run(repository: &Path, arguments: &[&str]) {
    let output = Command::new("git")
        .args(arguments)
        .current_dir(repository)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn branch(directory: &Path) -> String {
    let output = Command::new("git")
        .args(["branch", "--show-current"])
        .current_dir(directory)
        .output()
        .unwrap();
    assert!(output.status.success());
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}
