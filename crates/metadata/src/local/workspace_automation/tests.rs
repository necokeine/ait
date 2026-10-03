use std::fs;
use std::thread;

use super::*;

#[cfg(unix)]
mod paseo;

#[cfg(unix)]
#[test]
fn setup_output_limit_applies_to_fast_commands_that_exit_between_polls() {
    let root = tempfile::tempdir().unwrap();
    let runtime = LocalWorkspaceAutomation::default();
    let workspace = placement(root.path(), "output-limit");
    let result = run_setup_command(
        &Arc::downgrade(&runtime.inner),
        &workspace,
        "head -c 8388609 /dev/zero",
        3000,
    );
    assert!(
        matches!(result, Err(WorkspaceAutomationError::Io(ref message)) if message == "Setup command output exceeded 8 MiB")
    );
}

#[test]
fn invalid_setup_configuration_is_recorded_without_launching_commands() {
    let root = tempfile::tempdir().unwrap();
    let runtime = LocalWorkspaceAutomation::default();
    let workspace = placement(root.path(), "invalid");
    for content in ["[]".to_owned(), " ".repeat(CONFIG_BYTES + 1)] {
        fs::write(root.path().join("ait.json"), content).unwrap();
        assert!(matches!(
            runtime.start_setup(&workspace),
            Err(WorkspaceAutomationError::InvalidConfig(_))
        ));
        let snapshot = runtime.setup_snapshot(&workspace.workspace_id).unwrap();
        assert_eq!(snapshot.lifecycle, SetupLifecycle::Failed);
        assert!(snapshot.commands.is_empty());
        assert!(snapshot.error.is_some());
    }
    fs::remove_file(root.path().join("ait.json")).unwrap();
    fs::create_dir(root.path().join("ait.json")).unwrap();
    assert!(runtime.start_setup(&workspace).is_err());
    assert!(parse_setup(Some(&serde_json::json!({"setup":false}))).is_empty());
}

#[test]
fn setup_transitions_publish_running_and_completed_snapshots() {
    use std::sync::mpsc;

    let root = tempfile::tempdir().expect("setup root");
    let mut runtime = LocalWorkspaceAutomation::default();
    let workspace = placement(root.path(), "setup-events");
    let (sender, receiver) = mpsc::channel();
    runtime.set_event_sink(Arc::new(move |event| {
        sender.send(event).expect("setup receiver");
    }));
    for lifecycle in [SetupLifecycle::Running, SetupLifecycle::Completed] {
        publish_setup(
            &Arc::downgrade(&runtime.inner),
            &workspace,
            SetupProgress {
                lifecycle,
                log: "",
                commands: &[],
                truncated: false,
                error: None,
            },
        );
    }
    let mut statuses = Vec::new();
    for _ in 0..2 {
        let event = receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("setup event");
        let AutomationEvent::Setup {
            workspace_id,
            snapshot,
        } = event
        else {
            panic!("setup progress expected");
        };
        assert_eq!(workspace_id, "setup-events");
        statuses.push(snapshot.lifecycle);
    }
    assert!(statuses.contains(&SetupLifecycle::Running));
    assert_eq!(statuses.last(), Some(&SetupLifecycle::Completed));
}

#[test]
fn natural_script_exit_publishes_stopped_status() {
    use std::sync::mpsc;

    let root = tempfile::tempdir().expect("script root");
    let mut runtime = LocalWorkspaceAutomation::default();
    let workspace = placement(root.path(), "script-exit");
    let (sender, receiver) = mpsc::channel();
    runtime.set_event_sink(Arc::new(move |event| {
        sender.send(event).expect("script receiver");
    }));
    let child = shell_command("exit 0").spawn().expect("script child");
    lock(&runtime.inner.state).scripts.insert(
        (workspace.workspace_id.clone(), "once".to_owned()),
        ScriptProcess {
            kind: ScriptType::Script,
            hostname: "once".to_owned(),
            port: None,
            terminal_id: "terminal-once".to_owned(),
            child: Some(child),
            exit_code: None,
        },
    );
    monitor_script(
        &Arc::downgrade(&runtime.inner),
        &workspace,
        "once",
        Duration::from_millis(1),
    );
    let AutomationEvent::Scripts {
        workspace_id,
        scripts,
    } = receiver.try_recv().expect("stopped status")
    else {
        panic!("script status expected");
    };
    assert_eq!(workspace_id, "script-exit");
    assert_eq!(scripts.len(), 1);
    assert!(!scripts[0].running);
    assert_eq!(scripts[0].exit_code, Some(0));
}

#[cfg(unix)]
#[test]
fn removed_configuration_does_not_hide_running_scripts_and_drop_reaps_them() {
    let root = tempfile::tempdir().unwrap();
    let workspace = placement(root.path(), "service");
    let config = root.path().join("ait.json");
    fs::write(
        &config,
        r#"{"scripts":{"web":{"type":"service","command":"while :; do sleep 1; done"}}}"#,
    )
    .unwrap();
    let runtime = LocalWorkspaceAutomation::default();
    let started = runtime.start_script(&workspace, "web").unwrap();
    let pid = lock(&runtime.inner.state)
        .scripts
        .values()
        .next()
        .unwrap()
        .child
        .as_ref()
        .unwrap()
        .id();
    fs::write(&config, "{}").unwrap();
    assert_eq!(runtime.list_scripts(&workspace).unwrap(), [started]);
    assert!(matches!(
        runtime.start_script(&workspace, "absent"),
        Err(WorkspaceAutomationError::UnknownScript(_))
    ));
    assert!(matches!(
        runtime.stop_script(&workspace, "absent"),
        Err(WorkspaceAutomationError::NotRunning(_))
    ));
    drop(runtime);
    let output = Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "pid="])
        .output()
        .unwrap();
    assert!(
        output.stdout.is_empty(),
        "script remained alive after runtime drop"
    );
}

#[cfg(unix)]
#[test]
fn setup_spawn_failure_finishes_the_attempt_and_releases_running_state() {
    let root = tempfile::tempdir().unwrap();
    let workspace = placement(&root.path().join("removed"), "removed");
    let runtime = LocalWorkspaceAutomation::default();
    lock(&runtime.inner.state)
        .setup_running
        .insert(workspace.workspace_id.clone());
    run_setup(
        &Arc::downgrade(&runtime.inner),
        &workspace,
        vec!["printf never".into()],
        3000,
    );
    let result = runtime.setup_snapshot(&workspace.workspace_id).unwrap();
    assert_eq!(result.lifecycle, SetupLifecycle::Failed);
    assert_eq!(result.commands.len(), 1);
    assert!(!result.commands[0].running);
    assert!(result.commands[0].exit_code.is_none());
    assert!(
        result
            .error
            .unwrap()
            .contains("Workspace setup process failed")
    );
    assert!(
        !lock(&runtime.inner.state)
            .setup_running
            .contains(&workspace.workspace_id)
    );
}

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
        directory.path().join("ait.json"),
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
    fs::write(directory.path().join("ait.json"), "[").expect("config");
    let automation = LocalWorkspaceAutomation::default();
    assert!(matches!(
        automation.list_scripts(&placement(directory.path(), "wks_bad")),
        Err(WorkspaceAutomationError::InvalidConfig(_))
    ));

    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;

        fs::remove_file(directory.path().join("ait.json")).expect("remove config");
        fs::write(directory.path().join("outside.json"), "{}").expect("outside");
        symlink("outside.json", directory.path().join("ait.json")).expect("symlink");
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
        directory.path().join("ait.json"),
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
        directory.path().join("ait.json"),
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
        directory.path().join("ait.json"),
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
        directory.path().join("ait.json"),
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

#[test]
fn script_config_prefers_ait_and_reads_legacy_only_when_ait_is_absent() {
    use crate::ports::provisioning::{LEGACY_PROJECT_CONFIG_FILE_NAME, PROJECT_CONFIG_FILE_NAME};

    let directory = tempfile::tempdir().unwrap();
    fs::write(
        directory.path().join(LEGACY_PROJECT_CONFIG_FILE_NAME),
        r#"{"scripts":{"legacy":{"command":"echo old"}}}"#,
    )
    .unwrap();
    assert!(
        read_config(directory.path())
            .unwrap()
            .scripts
            .contains_key("legacy")
    );
    let preferred = directory.path().join(PROJECT_CONFIG_FILE_NAME);
    fs::write(
        &preferred,
        r#"{"scripts":{"ait":{"command":"echo current"}}}"#,
    )
    .unwrap();
    let config = read_config(directory.path()).unwrap();
    assert!(config.scripts.contains_key("ait"));
    assert!(!config.scripts.contains_key("legacy"));
    fs::write(&preferred, "invalid").unwrap();
    assert!(read_config(directory.path()).is_err());
}
