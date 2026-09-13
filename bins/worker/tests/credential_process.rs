//! Credential variables are removed before any worker or its descendants start.
#![cfg(unix)]
#![allow(clippy::pedantic)]
use std::os::unix::fs::PermissionsExt;
use std::{io::Write, process::Stdio};
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

#[test]
fn real_worker_protocol_diagnostic_never_echoes_sensitive_input() {
    let secrets = [
        "NEC248_PRIVATE_KEY_DIAGNOSTIC",
        "NEC248_ACCESS_KEY_DIAGNOSTIC",
        "NEC248_CREDENTIAL_DIAGNOSTIC",
        "NEC248_AUTH_DIAGNOSTIC",
        "NEC248_URI_DIAGNOSTIC",
        "NEC248_PEM_DIAGNOSTIC",
    ];
    let malformed = serde_json::json!({
        "private_key": secrets[0],
        "access_key": secrets[1],
        "credential": secrets[2],
        "auth": secrets[3],
        "uri": format!("https://user:{}@example.test", secrets[4]),
        "pem": format!("-----BEGIN PRIVATE KEY-----\n{}\n-----END PRIVATE KEY-----", secrets[5]),
    })
    .to_string();
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_ait-worker"))
        .args(["--stdio", "--protocol-major", "1"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    input
        .write_all(&u32::try_from(malformed.len()).unwrap().to_be_bytes())
        .unwrap();
    input.write_all(malformed.as_bytes()).unwrap();
    drop(input);
    let output = child.wait_with_output().unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("InvalidFrame"));
    for bytes in [&output.stdout, &output.stderr] {
        let diagnostic = String::from_utf8_lossy(bytes);
        assert!(!diagnostic.contains(&malformed));
        for secret in secrets {
            assert!(!diagnostic.contains(secret));
        }
    }
}
