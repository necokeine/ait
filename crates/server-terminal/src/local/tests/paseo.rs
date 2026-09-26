//! Local PTY cases from Paseo terminal.test.ts, using deterministic shell handshakes.

use super::*;

#[test]
fn native_terminal_keeps_metacharacters_literal_in_positional_arguments() {
    let root = tempfile::tempdir().unwrap();
    let literal = "a b; $(touch unexpected-file) `echo injected`";
    let mut config = launch(root.path(), "printf 'ARG:%s\\n' \"$1\"; read line");
    config
        .args
        .extend(["fixture".to_owned(), literal.to_owned()]);
    let mut process = LocalRuntime.spawn(&config).unwrap();
    let output = until(process.as_ref(), |text| text.contains("ARG:"));
    assert!(output.contains(&format!("ARG:{literal}")));
    assert!(!root.path().join("unexpected-file").exists());
    process.kill().unwrap();
}

#[test]
fn native_terminal_reports_osc_title_changes_without_visible_output() {
    let root = tempfile::tempdir().unwrap();
    let config = launch(
        root.path(),
        "printf '\\033]2;Build Log\\007READY\\n'; read line; printf '\\033]2;Finished\\007DONE\\n'; read line",
    );
    let mut process = LocalRuntime.spawn(&config).unwrap();
    until(process.as_ref(), |text| text.contains("READY"));
    assert_eq!(process.title().as_deref(), Some("Build Log"));
    process
        .send(&Input::Input {
            data: "continue\r".to_owned(),
        })
        .unwrap();
    until(process.as_ref(), |text| text.contains("DONE"));
    assert_eq!(process.title().as_deref(), Some("Finished"));
    assert!(!process.capture().unwrap().join("\n").contains("Build Log"));
    process.kill().unwrap();
}

#[test]
fn native_terminal_retains_final_rows_after_output_exceeds_the_recent_byte_budget() {
    let root = tempfile::tempdir().unwrap();
    let config = launch(
        root.path(),
        "/usr/bin/awk 'BEGIN { for (i=0; i<6000; i++) printf \"line-%04d-abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ\\n\", i; print \"LAST-LARGE-OUTPUT\" }'; read line",
    );
    let mut process = LocalRuntime.spawn(&config).unwrap();
    let output = until(process.as_ref(), |text| text.contains("LAST-LARGE-OUTPUT"));
    assert!(!output.contains("line-0000-"));
    assert!(output.contains("line-5999-"));
    assert!(process.capture().unwrap().len() <= 150);
    let recovered = process.observe(Some(0), None).unwrap();
    assert_eq!(recovered.frames.len(), 1);
    assert_eq!(recovered.frames[0].0, crate::protocol::Opcode::Snapshot);
    process.kill().unwrap();
}

#[test]
fn native_terminal_preserves_launch_failure_diagnostics_after_natural_exit() {
    let root = tempfile::tempdir().unwrap();
    let mut process = LocalRuntime
        .spawn(&launch(
            root.path(),
            "printf 'launch failed\\ncommand missing\\n'; exit 127",
        ))
        .unwrap();
    until(process.as_ref(), |text| text.contains("command missing"));
    let deadline = Instant::now() + Duration::from_secs(10);
    while !process.exited().unwrap() {
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(10));
    }
    let output = process.capture().unwrap().join("\n");
    assert!(output.contains("launch failed\ncommand missing"));
    assert!(process.observe(None, None).unwrap().exited);
}

#[test]
fn terminal_directory_resolves_symlinks_and_rejects_files_without_spawning() {
    let root = tempfile::tempdir().unwrap();
    let target = root.path().join("directory");
    std::fs::create_dir(&target).unwrap();
    let link = root.path().join("alias");
    std::os::unix::fs::symlink(&target, &link).unwrap();
    assert_eq!(
        LocalRuntime.directory(link.to_str().unwrap()).unwrap(),
        target.canonicalize().unwrap().to_str().unwrap()
    );
    let file = root.path().join("file");
    std::fs::write(&file, b"not a directory").unwrap();
    assert_eq!(
        LocalRuntime.directory(file.to_str().unwrap()),
        Err(Error::Invalid)
    );
}
