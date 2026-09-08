//! Cleanup of a child that stalls before completing the protocol handshake.
#![cfg(unix)]

use std::{fs, os::unix::fs::PermissionsExt, time::Duration};

use ait_agent_adapters::{
    AdapterErrorKind, AgentAdapter, AgentRunRequest, ApprovalPolicy, SandboxMode,
    codex::{CodexAppServerAdapter, CodexAppServerConfig},
};
use tokio_stream::StreamExt;
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn cancellation_during_handshake_reaps_the_owned_child() {
    let directory = tempfile::tempdir().unwrap();
    let cwd = directory.path().canonicalize().unwrap();
    let binary = cwd.join("stalled-codex");
    // Only shell builtins: no grandchildren or external sleep process to orphan.
    fs::write(
        &binary,
        "#!/bin/sh\necho $$ > child.pid\nwhile read -r line; do :; done\n",
    )
    .unwrap();
    fs::set_permissions(&binary, fs::Permissions::from_mode(0o700)).unwrap();
    let adapter = CodexAppServerAdapter::new(CodexAppServerConfig {
        codex_binary: binary,
        ..CodexAppServerConfig::default()
    })
    .unwrap();
    let cancellation = CancellationToken::new();
    let mut stream = adapter
        .run(AgentRunRequest {
            request_id: "cancel-startup".into(),
            model: None,
            reasoning_effort: None,
            project_instructions: None,
            prompt: "hello".into(),
            cwd: cwd.clone(),
            resume_thread_id: None,
            sandbox: SandboxMode::ReadOnly,
            approval_policy: ApprovalPolicy::Never,
            output_schema: None,
            approval_handler: None,
            cancellation: cancellation.clone(),
        })
        .await
        .unwrap();
    let pid = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Ok(pid) = fs::read_to_string(cwd.join("child.pid"))
                && !pid.trim().is_empty()
            {
                break pid;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    cancellation.cancel();
    tokio::time::timeout(Duration::from_secs(5), async {
        let error = stream.next().await.unwrap().unwrap_err();
        assert_eq!(error.kind, AdapterErrorKind::Cancelled);
        // Closing the stream signals that child.wait() has completed.
        assert!(stream.next().await.is_none());
    })
    .await
    .expect("cancel must also work before initialize responds");
    let alive = std::process::Command::new("kill")
        .args(["-0", pid.trim()])
        .output()
        .unwrap();
    assert!(!alive.status.success(), "the adapter must reap its child");
}
