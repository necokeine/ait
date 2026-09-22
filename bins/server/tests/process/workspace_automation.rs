use std::path::Path;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::transport::{Socket, connect, request};
use super::{ready, start, terminate};

#[tokio::test]
async fn binary_runs_canonical_workspace_setup_and_script_methods() {
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    std::fs::write(
        workspace.join("paseo.json"),
        r#"{
          "worktree":{"setup":["printf setup > setup.txt"]},
          "scripts":{
            "once":{"command":"printf script > script.txt"},
            "service":{"type":"service","command":"while :; do sleep 1; done"}
          }
        }"#,
    )
    .unwrap();
    let state = root.path().join("server");
    seed_registry(&state, &workspace);
    let log = root.path().join("server.log");
    let mut process = start(&state, &log);
    let address = ready(&mut process, &log).await;
    let mut client = connect(
        &address,
        server_protocol::workspace_automation::CAPABILITIES,
    )
    .await;

    assert_blocked_and_approve(&mut client, &workspace).await;
    assert_script_lifecycle(&mut client, &workspace).await;
    assert_legacy_method_rejected(&mut client).await;

    terminate(&mut process).await;
    let records: Value =
        serde_json::from_slice(&std::fs::read(state.join("projects/workspaces.json")).unwrap())
            .unwrap();
    assert!(records[0].get("untrustedSource").is_none());
}

async fn assert_blocked_and_approve(client: &mut Socket, workspace: &Path) {
    let blocked = request(
        client,
        "workspace.setup.status.request",
        json!({"workspaceId":"wks_automation"}),
    )
    .await;
    assert_eq!(blocked["result"]["snapshot"]["status"], "blocked");
    assert_eq!(
        blocked["result"]["snapshot"]["blockedSource"]["kind"],
        "change_request"
    );
    assert_eq!(blocked["result"]["snapshot"]["blockedSource"]["number"], 42);

    let scripts = request(
        client,
        "workspace.script.list.request",
        json!({"workspaceId":"wks_automation"}),
    )
    .await;
    assert_eq!(scripts["result"]["scripts"][0]["scriptName"], "once");
    assert_eq!(scripts["result"]["scripts"][1]["scriptName"], "service");
    let denied = request(
        client,
        "workspace.script.start.request",
        json!({"workspaceId":"wks_automation","scriptName":"once"}),
    )
    .await;
    assert!(
        denied["result"]["error"]
            .as_str()
            .unwrap()
            .contains("blocked")
    );

    let approved = request(
        client,
        "workspace.setup.run.request",
        json!({"workspaceId":"wks_automation"}),
    )
    .await;
    assert_eq!(approved["result"]["started"], true, "{approved}");
    let setup = wait_for_setup(client, "completed").await;
    assert_eq!(setup["detail"]["commands"][0]["status"], "completed");
    assert_eq!(
        std::fs::read_to_string(workspace.join("setup.txt")).unwrap(),
        "setup"
    );
    assert_eq!(
        request(
            client,
            "workspace.setup.run.request",
            json!({"workspaceId":"wks_automation"})
        )
        .await["result"]["started"],
        false
    );
}

async fn assert_script_lifecycle(client: &mut Socket, workspace: &Path) {
    let once = request(
        client,
        "workspace.script.start.request",
        json!({"workspaceId":"wks_automation","scriptName":"once"}),
    )
    .await;
    assert_eq!(once["result"]["script"]["lifecycle"], "running");
    assert!(once["result"]["script"]["terminalId"].is_string());
    wait_for_script(client, "once", "stopped").await;
    assert_eq!(
        std::fs::read_to_string(workspace.join("script.txt")).unwrap(),
        "script"
    );

    let service = request(
        client,
        "workspace.script.start.request",
        json!({"workspaceId":"wks_automation","scriptName":"service"}),
    )
    .await;
    assert_eq!(service["result"]["script"]["type"], "service");
    assert_eq!(service["result"]["script"]["lifecycle"], "running");
    assert!(service["result"]["script"]["port"].is_number());
    let stopped = request(
        client,
        "workspace.script.stop.request",
        json!({"workspaceId":"wks_automation","scriptName":"service"}),
    )
    .await;
    assert_eq!(stopped["result"]["script"]["lifecycle"], "stopped");
}

async fn assert_legacy_method_rejected(client: &mut Socket) {
    let legacy = request(
        client,
        "workspace_setup_status_request",
        json!({"workspaceId":"wks_automation"}),
    )
    .await;
    assert_eq!(legacy["code"], "method_not_found");
}

async fn wait_for_setup(client: &mut Socket, expected: &str) -> Value {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let response = request(
            client,
            "workspace.setup.status.request",
            json!({"workspaceId":"wks_automation"}),
        )
        .await;
        let snapshot = response["result"]["snapshot"].clone();
        if snapshot["status"] == expected {
            return snapshot;
        }
        assert!(Instant::now() < deadline, "setup timeout: {response}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

async fn wait_for_script(client: &mut Socket, name: &str, expected: &str) -> Value {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let response = request(
            client,
            "workspace.script.list.request",
            json!({"workspaceId":"wks_automation"}),
        )
        .await;
        let script = response["result"]["scripts"]
            .as_array()
            .unwrap()
            .iter()
            .find(|script| script["scriptName"] == name)
            .unwrap()
            .clone();
        if script["lifecycle"] == expected {
            return script;
        }
        assert!(Instant::now() < deadline, "script timeout: {response}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

fn seed_registry(state: &Path, workspace: &Path) {
    let registry = state.join("projects");
    std::fs::create_dir_all(&registry).unwrap();
    std::fs::write(
        registry.join("projects.json"),
        serde_json::to_vec_pretty(&json!([{
            "projectId":"prj_automation",
            "rootPath":workspace,
            "kind":"non_git",
            "displayName":"Automation",
            "projectKey":null,
            "customName":null,
            "customIconRevision":null,
            "createdAt":"2026-01-01T00:00:00.000Z",
            "updatedAt":"2026-01-01T00:00:00.000Z",
            "archivedAt":null
        }]))
        .unwrap(),
    )
    .unwrap();
    std::fs::write(
        registry.join("workspaces.json"),
        serde_json::to_vec_pretty(&json!([{
            "workspaceId":"wks_automation",
            "projectId":"prj_automation",
            "cwd":workspace,
            "kind":"directory",
            "displayName":"Automation",
            "title":null,
            "branch":"feature/test",
            "worktreeRoot":workspace,
            "baseBranch":null,
            "isPaseoOwnedWorktree":false,
            "mainRepoRoot":workspace,
            "createdAt":"2026-01-01T00:00:00.000Z",
            "updatedAt":"2026-01-01T00:00:00.000Z",
            "archivedAt":null,
            "autoArchivedChangeRequestUrl":null,
            "pinnedAt":null,
            "labels":[],
            "untrustedSource":{
                "kind":"change_request",
                "forge":"github",
                "number":42,
                "headRepository":"fork/repo"
            }
        }]))
        .unwrap(),
    )
    .unwrap();
}
