use std::fs;
use std::thread;

use super::*;

fn placement(root: &Path, id: &str) -> WorkspacePlacement {
    WorkspacePlacement {
        workspace_id: id.to_owned(),
        cwd: root.to_string_lossy().into_owned(),
        worktree_path: root.to_string_lossy().into_owned(),
        repo_root: root.to_string_lossy().into_owned(),
        branch_name: "feature/test".to_owned(),
    }
}

fn wait_for(
    automation: &LocalWorkspaceAutomation,
    workspace_id: &str,
    expected: SetupLifecycle,
) -> SetupSnapshot {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let snapshot = automation
            .setup_snapshot(workspace_id)
            .expect("setup snapshot");
        if snapshot.lifecycle == expected {
            return snapshot;
        }
        assert!(
            Instant::now() < deadline,
            "setup did not finish: {snapshot:?}"
        );
        thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn script_config_filters_invalid_entries_and_sorts_names() {
    let directory = tempfile::tempdir().expect("tempdir");
    fs::write(
        directory.path().join("paseo.json"),
        r#"{
          "scripts": {
            "zeta": {"command":" echo z "},
            "Alpha2": {"type":"service","command":"run","port":4321},
            "Alpha10": {"type":"worker","command":"work"},
            "missing": {"type":"service"},
            "blank": {"command":"  "},
            "text": "echo no"
          }
        }"#,
    )
    .expect("config");
    let automation = LocalWorkspaceAutomation::default();
    let scripts = automation
        .list_scripts(&placement(directory.path(), "wks_list"))
        .expect("list");
    assert_eq!(
        scripts
            .iter()
            .map(|script| script.name.as_str())
            .collect::<Vec<_>>(),
        ["Alpha10", "Alpha2", "zeta"]
    );
    assert_eq!(scripts[0].kind, ScriptType::Script);
    assert_eq!(scripts[1].kind, ScriptType::Service);
    assert_eq!(scripts[1].port, Some(4321));
    assert!(scripts.iter().all(|script| !script.running));
}

#[test]
fn malformed_and_linked_configs_are_rejected() {
    let directory = tempfile::tempdir().expect("tempdir");
    fs::write(directory.path().join("paseo.json"), "[").expect("config");
    let automation = LocalWorkspaceAutomation::default();
    assert!(matches!(
        automation.list_scripts(&placement(directory.path(), "wks_bad")),
        Err(WorkspaceAutomationError::InvalidConfig(_))
    ));

    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;

        fs::remove_file(directory.path().join("paseo.json")).expect("remove config");
        fs::write(directory.path().join("outside.json"), "{}").expect("outside");
        symlink("outside.json", directory.path().join("paseo.json")).expect("symlink");
        assert!(matches!(
            automation.list_scripts(&placement(directory.path(), "wks_link")),
            Err(WorkspaceAutomationError::InvalidConfig(_))
        ));
    }
}

#[cfg(unix)]
#[test]
fn scripts_start_refresh_and_stop_real_children() {
    let directory = tempfile::tempdir().expect("tempdir");
    fs::write(
        directory.path().join("paseo.json"),
        r#"{"scripts":{
          "once":{"command":"printf done > once.txt"},
          "service":{"type":"service","command":"while :; do sleep 1; done"}
        }}"#,
    )
    .expect("config");
    let workspace = placement(directory.path(), "wks_process");
    let automation = LocalWorkspaceAutomation::default();
    let started = automation
        .start_script(&workspace, "once")
        .expect("start once");
    assert!(started.running);
    assert!(started.terminal_id.is_some());
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let once = automation
            .list_scripts(&workspace)
            .expect("list")
            .into_iter()
            .find(|script| script.name == "once")
            .expect("once");
        if !once.running {
            assert_eq!(once.exit_code, Some(0));
            break;
        }
        assert!(Instant::now() < deadline, "one-shot script did not exit");
        thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(
        fs::read_to_string(directory.path().join("once.txt")).expect("marker"),
        "done"
    );
    let service = automation
        .start_script(&workspace, "service")
        .expect("start service");
    assert!(service.running);
    assert!(service.port.is_some());
    assert!(matches!(
        automation.start_script(&workspace, "service"),
        Err(WorkspaceAutomationError::AlreadyRunning(_))
    ));
    let stopped = automation
        .stop_script(&workspace, "service")
        .expect("stop service");
    assert!(!stopped.running);
    assert!(matches!(
        automation.stop_script(&workspace, "service"),
        Err(WorkspaceAutomationError::NotRunning(_))
    ));
}

#[cfg(unix)]
#[test]
fn setup_runs_in_order_with_paseo_environment() {
    let directory = tempfile::tempdir().expect("tempdir");
    fs::write(
        directory.path().join("paseo.json"),
        r#"{"worktree":{"setup":[
          "printf '%s' \"$PASEO_BRANCH_NAME\" > branch.txt",
          "printf '%s' \"$PASEO_WORKTREE_PORT\" > port.txt"
        ]}}"#,
    )
    .expect("config");
    let workspace = placement(directory.path(), "wks_setup");
    let automation = LocalWorkspaceAutomation::default();
    assert!(automation.start_setup(&workspace).expect("start"));
    assert!(!automation.start_setup(&workspace).expect("deduplicate"));
    let snapshot = wait_for(&automation, "wks_setup", SetupLifecycle::Completed);
    assert_eq!(snapshot.commands.len(), 2);
    assert!(
        snapshot
            .commands
            .iter()
            .all(|command| command.exit_code == Some(0))
    );
    assert_eq!(
        fs::read_to_string(directory.path().join("branch.txt")).expect("branch"),
        "feature/test"
    );
    assert!(
        fs::read_to_string(directory.path().join("port.txt"))
            .expect("port")
            .parse::<u16>()
            .is_ok()
    );
}

#[cfg(unix)]
#[test]
fn setup_stops_after_the_first_failed_command() {
    let directory = tempfile::tempdir().expect("tempdir");
    fs::write(
        directory.path().join("paseo.json"),
        r#"{"worktree":{"setup":["printf before; exit 7","touch after.txt"]}}"#,
    )
    .expect("config");
    let workspace = placement(directory.path(), "wks_failure");
    let automation = LocalWorkspaceAutomation::default();
    assert!(automation.start_setup(&workspace).expect("start"));
    let snapshot = wait_for(&automation, "wks_failure", SetupLifecycle::Failed);
    assert_eq!(snapshot.commands.len(), 1);
    assert_eq!(snapshot.commands[0].exit_code, Some(7));
    assert_eq!(snapshot.commands[0].log, "before");
    assert!(!directory.path().join("after.txt").exists());
}

#[cfg(unix)]
#[test]
fn setup_thread_does_not_retain_the_runtime_across_restart() {
    let directory = tempfile::tempdir().expect("tempdir");
    fs::write(
        directory.path().join("paseo.json"),
        r#"{"worktree":{"setup":["sleep 5"]}}"#,
    )
    .expect("config");
    let workspace = placement(directory.path(), "wks_restart");
    let automation = LocalWorkspaceAutomation::default();
    let inner = Arc::downgrade(&automation.inner);
    assert!(automation.start_setup(&workspace).expect("start"));
    drop(automation);
    assert!(inner.upgrade().is_none());
}
