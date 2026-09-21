//! Real binary startup, environment handling, directory exclusion, and Unix signal shutdown.

use std::process::Command;

#[test]
fn help_and_missing_credentials_do_not_create_state() {
    let directory = tempfile::tempdir().unwrap();
    let state = directory.path().join("state");
    let help = Command::new(env!("CARGO_BIN_EXE_server"))
        .arg("--help")
        .env_remove("AIT_SERVER_TOKEN")
        .output()
        .unwrap();
    assert!(help.status.success());
    assert!(String::from_utf8_lossy(&help.stdout).contains("--data-dir"));
    let failure = Command::new(env!("CARGO_BIN_EXE_server"))
        .args(["--data-dir", state.to_str().unwrap()])
        .env_remove("AIT_SERVER_TOKEN")
        .output()
        .unwrap();
    assert!(!failure.status.success());
    assert!(String::from_utf8_lossy(&failure.stderr).contains("AIT_SERVER_TOKEN"));
    assert!(!state.exists());
}

#[cfg(unix)]
#[path = "process/unix.rs"]
mod unix;
