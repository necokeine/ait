//! Paseo workspace scripts/runtime-store and setup behavior at the process boundary.

use serde_json::json;

use super::*;

fn config(root: &Path, value: &Value) {
    fs::write(root.join("paseo.json"), serde_json::to_vec(value).unwrap()).unwrap();
}

fn wait_script(
    automation: &LocalWorkspaceAutomation,
    workspace: &WorkspacePlacement,
) -> ScriptSnapshot {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let snapshot = automation.list_scripts(workspace).unwrap().remove(0);
        if !snapshot.running {
            return snapshot;
        }
        assert!(
            Instant::now() < deadline,
            "script did not finish: {snapshot:?}"
        );
        thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn missing_config_returns_no_scripts_without_creating_a_file() {
    let root = tempfile::tempdir().unwrap();
    let automation = LocalWorkspaceAutomation::default();
    assert!(
        automation
            .list_scripts(&placement(root.path(), "missing"))
            .unwrap()
            .is_empty()
    );
    assert!(!root.path().join("paseo.json").exists());
}

#[test]
fn same_cwd_workspaces_run_and_stop_scripts_independently() {
    let root = tempfile::tempdir().unwrap();
    config(
        root.path(),
        &json!({"scripts":{"serve":{"type":"service","command":"while :; do sleep 1; done"}}}),
    );
    let automation = LocalWorkspaceAutomation::default();
    let first = placement(root.path(), "first");
    let second = placement(root.path(), "second");
    let one = automation.start_script(&first, "serve").unwrap();
    let two = automation.start_script(&second, "serve").unwrap();
    assert_ne!(one.terminal_id, two.terminal_id);
    assert_ne!(one.port, two.port);
    automation.stop_script(&first, "serve").unwrap();
    assert!(!automation.list_scripts(&first).unwrap()[0].running);
    assert!(automation.list_scripts(&second).unwrap()[0].running);
    automation.stop_script(&second, "serve").unwrap();
}

#[test]
fn plain_script_does_not_expose_a_service_port_from_config() {
    let root = tempfile::tempdir().unwrap();
    config(
        root.path(),
        &json!({"scripts":{"build":{"command":"exit 0","port":4321}}}),
    );
    let automation = LocalWorkspaceAutomation::default();
    let workspace = placement(root.path(), "build");
    let configured = automation.list_scripts(&workspace).unwrap().remove(0);
    assert_eq!(configured.kind, ScriptType::Script);
    assert_eq!(configured.port, None);
    let started = automation.start_script(&workspace, "build").unwrap();
    assert_eq!(started.kind, ScriptType::Script);
    assert_eq!(started.port, None);
    assert_eq!(wait_script(&automation, &workspace).exit_code, Some(0));
}

#[test]
fn service_keeps_its_explicit_port_and_kind_after_exit() {
    let root = tempfile::tempdir().unwrap();
    config(
        root.path(),
        &json!({"scripts":{"serve":{"type":"service","command":"exit 9","port":4321}}}),
    );
    let automation = LocalWorkspaceAutomation::default();
    let workspace = placement(root.path(), "serve");
    let started = automation.start_script(&workspace, "serve").unwrap();
    assert_eq!(started.port, Some(4321));
    let finished = wait_script(&automation, &workspace);
    assert_eq!(finished.kind, ScriptType::Service);
    assert_eq!(finished.port, Some(4321));
    assert_eq!(finished.exit_code, Some(9));
    assert_eq!(finished.terminal_id, started.terminal_id);
}

#[test]
fn exited_script_can_restart_with_a_new_terminal_identity() {
    let root = tempfile::tempdir().unwrap();
    config(
        root.path(),
        &json!({"scripts":{"once":{"command":"printf x >> executions"}}}),
    );
    let automation = LocalWorkspaceAutomation::default();
    let workspace = placement(root.path(), "restart");
    let first = automation.start_script(&workspace, "once").unwrap();
    assert_eq!(wait_script(&automation, &workspace).exit_code, Some(0));
    let second = automation.start_script(&workspace, "once").unwrap();
    assert_ne!(first.terminal_id, second.terminal_id);
    assert_eq!(wait_script(&automation, &workspace).exit_code, Some(0));
    assert_eq!(
        fs::read_to_string(root.path().join("executions")).unwrap(),
        "xx"
    );
}

#[test]
fn removing_script_configuration_keeps_the_live_process_stoppable() {
    let root = tempfile::tempdir().unwrap();
    config(
        root.path(),
        &json!({"scripts":{"serve":{"command":"while :; do sleep 1; done"}}}),
    );
    let automation = LocalWorkspaceAutomation::default();
    let workspace = placement(root.path(), "removed-config");
    let started = automation.start_script(&workspace, "serve").unwrap();
    config(root.path(), &json!({"scripts":{}}));
    let listed = automation.list_scripts(&workspace).unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].terminal_id, started.terminal_id);
    assert!(listed[0].running);
    assert!(!automation.stop_script(&workspace, "serve").unwrap().running);
    assert!(matches!(
        automation.start_script(&workspace, "serve"),
        Err(WorkspaceAutomationError::UnknownScript(_))
    ));
}

#[test]
fn unknown_script_does_not_spawn_or_pollute_another_workspace() {
    let root = tempfile::tempdir().unwrap();
    config(
        root.path(),
        &json!({"scripts":{"known":{"command":"touch unexpected"}}}),
    );
    let automation = LocalWorkspaceAutomation::default();
    let workspace = placement(root.path(), "unknown");
    assert!(matches!(
        automation.start_script(&workspace, "missing"),
        Err(WorkspaceAutomationError::UnknownScript(_))
    ));
    assert!(!root.path().join("unexpected").exists());
    assert_eq!(
        automation.list_scripts(&workspace).unwrap()[0].terminal_id,
        None
    );
}

#[test]
fn malformed_setup_config_publishes_a_failed_snapshot_without_running_commands() {
    let root = tempfile::tempdir().unwrap();
    config(root.path(), &json!(["touch unexpected"]));
    let automation = LocalWorkspaceAutomation::default();
    assert!(matches!(
        automation.start_setup(&placement(root.path(), "bad")),
        Err(WorkspaceAutomationError::InvalidConfig(_))
    ));
    let snapshot = automation.setup_snapshot("bad").unwrap();
    assert_eq!(snapshot.lifecycle, SetupLifecycle::Failed);
    assert!(snapshot.error.is_some());
    assert!(snapshot.commands.is_empty());
    assert!(!root.path().join("unexpected").exists());
}

#[test]
fn setup_accepts_one_command_and_filters_blank_or_non_string_commands() {
    let root = tempfile::tempdir().unwrap();
    let automation = LocalWorkspaceAutomation::default();
    config(
        root.path(),
        &json!({"worktree":{"setup":"printf first > sequence"}}),
    );
    assert!(
        automation
            .start_setup(&placement(root.path(), "one"))
            .unwrap()
    );
    assert_eq!(
        wait_for(&automation, "one", SetupLifecycle::Completed)
            .commands
            .len(),
        1
    );
    config(
        root.path(),
        &json!({"worktree":{"setup":[null, "", "  ", 12, "printf second >> sequence"]}}),
    );
    assert!(
        automation
            .start_setup(&placement(root.path(), "two"))
            .unwrap()
    );
    assert_eq!(
        wait_for(&automation, "two", SetupLifecycle::Completed)
            .commands
            .len(),
        1
    );
    assert_eq!(
        fs::read_to_string(root.path().join("sequence")).unwrap(),
        "firstsecond"
    );
}

#[test]
fn setup_environment_uses_saved_checkout_and_repository_placement() {
    let root = tempfile::tempdir().unwrap();
    config(
        root.path(),
        &json!({"worktree":{"setup":"printf '%s\\n' \"$PASEO_WORKTREE_PATH\" \"$PASEO_SOURCE_CHECKOUT_PATH\" \"$PASEO_BRANCH_NAME\" > environment"}}),
    );
    let mut workspace = placement(root.path(), "environment");
    workspace.worktree_path = "/saved/checkout".to_owned();
    workspace.repo_root = "/saved/repository".to_owned();
    let automation = LocalWorkspaceAutomation::default();
    automation.start_setup(&workspace).unwrap();
    wait_for(&automation, "environment", SetupLifecycle::Completed);
    assert_eq!(
        fs::read_to_string(root.path().join("environment")).unwrap(),
        "/saved/checkout\n/saved/repository\nfeature/test\n"
    );
}
