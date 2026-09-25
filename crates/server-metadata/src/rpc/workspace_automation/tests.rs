use crate::service::workspace_automation::{ScriptType, SetupCommandSnapshot};

use super::*;

#[test]
fn setup_projection_matches_running_completed_and_failed_commands() {
    let projected = setup_snapshot(
        SetupSnapshot {
            lifecycle: SetupLifecycle::Failed,
            worktree_path: "/tmp/worktree".to_owned(),
            branch_name: "feature/test".to_owned(),
            log: "all output".to_owned(),
            commands: vec![
                SetupCommandSnapshot {
                    index: 1,
                    command: "first".to_owned(),
                    cwd: "/tmp/worktree".to_owned(),
                    log: String::new(),
                    running: true,
                    exit_code: None,
                    duration_ms: None,
                },
                SetupCommandSnapshot {
                    index: 2,
                    command: "second".to_owned(),
                    cwd: "/tmp/worktree".to_owned(),
                    log: String::new(),
                    running: false,
                    exit_code: Some(0),
                    duration_ms: Some(2),
                },
                SetupCommandSnapshot {
                    index: 3,
                    command: "third".to_owned(),
                    cwd: "/tmp/worktree".to_owned(),
                    log: "bad".to_owned(),
                    running: false,
                    exit_code: Some(7),
                    duration_ms: Some(3),
                },
            ],
            truncated: false,
            error: Some("failed".to_owned()),
        },
        None,
    );
    assert_eq!(projected.status, WorkspaceSetupStatus::Failed);
    assert_eq!(
        projected
            .detail
            .commands
            .iter()
            .map(|command| command.status)
            .collect::<Vec<_>>(),
        [
            WorkspaceSetupCommandStatus::Running,
            WorkspaceSetupCommandStatus::Completed,
            WorkspaceSetupCommandStatus::Failed,
        ]
    );
}

#[test]
fn script_projection_keeps_runtime_identity_and_omits_unimplemented_proxy_state() {
    let projected = script(ScriptSnapshot {
        name: "web".to_owned(),
        kind: ScriptType::Service,
        hostname: "web".to_owned(),
        port: Some(4321),
        running: true,
        exit_code: None,
        terminal_id: Some("terminal-1".to_owned()),
    });
    assert_eq!(projected.kind, WorkspaceScriptType::Service);
    assert_eq!(projected.lifecycle, WorkspaceScriptLifecycle::Running);
    assert_eq!(projected.port, Some(4321));
    assert_eq!(projected.terminal_id.as_deref(), Some("terminal-1"));
    assert!(projected.local_proxy_url.is_none());
    assert!(projected.proxy_url.is_none());
}

#[test]
fn untrusted_change_request_maps_without_losing_provenance() {
    let projected = blocked_source(UntrustedWorkspaceSource::ChangeRequest {
        forge: "github".to_owned(),
        number: 42,
        head_repository: "fork/repo".to_owned(),
    });
    assert_eq!(
        projected,
        WorkspaceBlockedSource::ChangeRequest {
            forge: "github".to_owned(),
            number: 42,
            head_repository: "fork/repo".to_owned(),
        }
    );
}
