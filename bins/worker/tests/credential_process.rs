//! Credential variables are removed before any worker or its descendants start.
#![cfg(unix)]
#![allow(clippy::pedantic)]
use std::os::unix::fs::PermissionsExt;
use tokio::io::AsyncReadExt;

#[tokio::test]
async fn credential_environment_probe_child() {
    let Ok(binary) = std::env::var("AIT_TEST_ENV_PROBE") else {
        return;
    };
    let mut child = ait_sandbox::spawn_worker(std::path::Path::new(&binary)).unwrap();
    let mut output = String::new();
    child
        .stdout()
        .take()
        .unwrap()
        .take(65536)
        .read_to_string(&mut output)
        .await
        .unwrap();
    let report: serde_json::Value = serde_json::from_str(&output).unwrap();
    assert_eq!(
        report["argv"],
        serde_json::json!(["--stdio", "--protocol-major", "1"])
    );
    for key in [
        "OPENAI_API_KEY",
        "DEEPSEEK_API_KEY",
        "CODEX_API_KEY",
        "AIT_TEST_ENV_PROBE",
        "DATABASE_URL",
    ] {
        assert!(
            report["env"].get(key).is_none(),
            "credential environment variable survived"
        );
    }
    assert!(!output.contains("NEC248-test-secret"));
    assert!(child.wait().await.unwrap().success());
}

#[test]
fn worker_argv_environment_and_stderr_do_not_expose_grants() {
    let directory = tempfile::tempdir().unwrap();
    let binary = directory.path().join("probe");
    std::fs::write(
        &binary,
        r#"#!/usr/bin/env python3
import os,sys,json
print(json.dumps(dict(argv=sys.argv[1:],env=dict(os.environ))))
print('NEC248-test-secret',file=sys.stderr)
"#,
    )
    .unwrap();
    std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();
    let result = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "credential_environment_probe_child",
            "--nocapture",
        ])
        .env("AIT_TEST_ENV_PROBE", &binary)
        .env("OPENAI_API_KEY", "NEC248-test-secret")
        .env("DEEPSEEK_API_KEY", "NEC248-test-secret")
        .env("CODEX_API_KEY", "NEC248-test-secret")
        .env("DATABASE_URL", "NEC248-test-secret")
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    for bytes in [&result.stdout, &result.stderr] {
        assert!(!String::from_utf8_lossy(bytes).contains("NEC248-test-secret"));
    }
}
