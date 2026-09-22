use serde_json::json;

use super::*;

#[test]
fn capabilities_merge_legacy_setup_and_script_names_into_canonical_methods() {
    assert_eq!(
        CAPABILITIES,
        [
            "workspace.setup.status.request",
            "workspace.setup.run.request",
            "workspace.script.list.request",
            "workspace.script.start.request",
            "workspace.script.stop.request"
        ]
    );
}

#[test]
fn requests_use_paseo_camel_case_fields_and_strip_unknown_fields() {
    let setup: WorkspaceSetupRequest = serde_json::from_value(json!({
        "workspaceId":"ws-1",
        "future":true
    }))
    .expect("setup request");
    assert_eq!(setup.workspace_id, "ws-1");

    let script: WorkspaceScriptRequest = serde_json::from_value(json!({
        "workspaceId":"ws-1",
        "scriptName":"web",
        "future":true
    }))
    .expect("script request");
    assert_eq!(script.script_name, "web");
}

#[test]
fn setup_snapshot_matches_paseo_worktree_detail_shape() {
    let value = serde_json::to_value(WorkspaceSetupSnapshot {
        status: WorkspaceSetupStatus::Completed,
        detail: WorkspaceSetupDetail {
            kind: "worktree_setup".to_owned(),
            worktree_path: "/repo/worktree".to_owned(),
            branch_name: "feature".to_owned(),
            log: "done".to_owned(),
            commands: vec![WorkspaceSetupCommand {
                index: 1,
                command: "echo done".to_owned(),
                cwd: "/repo/worktree".to_owned(),
                log: "done\n".to_owned(),
                status: WorkspaceSetupCommandStatus::Completed,
                exit_code: Some(0),
                duration_ms: Some(12),
            }],
            truncated: false,
        },
        error: None,
        blocked_source: None,
    })
    .expect("snapshot");
    assert_eq!(value["status"], "completed");
    assert_eq!(value["detail"]["type"], "worktree_setup");
    assert!(value["detail"].get("truncated").is_none());
}

#[test]
fn script_payload_materializes_paseo_defaults() {
    let value = serde_json::to_value(WorkspaceScript {
        script_name: "dev".to_owned(),
        kind: WorkspaceScriptType::Service,
        hostname: "dev".to_owned(),
        port: Some(3000),
        local_proxy_url: None,
        public_proxy_url: None,
        proxy_url: None,
        lifecycle: WorkspaceScriptLifecycle::Stopped,
        health: None,
        exit_code: None,
        terminal_id: None,
    })
    .expect("script");
    assert_eq!(value["type"], "service");
    assert_eq!(value["lifecycle"], "stopped");
    assert!(value["proxyUrl"].is_null());
    assert!(value["exitCode"].is_null());
    assert!(value["terminalId"].is_null());
    assert!(value.get("localProxyUrl").is_none());
}
