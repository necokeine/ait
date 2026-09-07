//! Run-owned process-tree cleanup at startup and after interrupt timeout.
#![cfg(unix)]

use std::{fs, os::unix::fs::PermissionsExt, time::Duration};

use ait_agent_adapters::{
    AdapterErrorKind, AgentAdapter, AgentEvent, AgentRunRequest, ApprovalPolicy, SandboxMode,
    codex::{CodexAppServerAdapter, CodexAppServerConfig},
};
use tokio_stream::StreamExt;
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn cancellation_during_handshake_reaps_the_owned_child() {
    let directory = tempfile::tempdir().unwrap();
    let cwd = directory.path().canonicalize().unwrap();
    let binary = cwd.join("stalled-codex");
    fs::write(
        &binary,
        "#!/bin/sh\necho $$ > child.pid\nsleep 300 &\necho $! > grandchild.pid\nwhile read -r line; do :; done\n",
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
            cancellation: cancellation.clone(),
        })
        .await
        .unwrap();
    let (pid, grandchild_pid) = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let (Ok(pid), Ok(grandchild_pid)) = (
                fs::read_to_string(cwd.join("child.pid")),
                fs::read_to_string(cwd.join("grandchild.pid")),
            ) && !pid.trim().is_empty()
                && !grandchild_pid.trim().is_empty()
            {
                break (pid, grandchild_pid);
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let mut unrelated = std::process::Command::new("sleep")
        .arg("300")
        .spawn()
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
    for owned_pid in [pid.trim(), grandchild_pid.trim()] {
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                let alive = std::process::Command::new("kill")
                    .args(["-0", owned_pid])
                    .output()
                    .unwrap();
                if !alive.status.success() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("the adapter must reclaim its entire owned process group");
    }
    assert!(
        unrelated.try_wait().unwrap().is_none(),
        "process-group cleanup must not kill pre-existing processes"
    );
    unrelated.kill().unwrap();
    unrelated.wait().unwrap();
}

#[tokio::test]
async fn missing_interrupt_terminal_forces_the_owned_process_tree_to_exit() {
    let directory = tempfile::tempdir().unwrap();
    let cwd = directory.path().canonicalize().unwrap();
    let binary = cwd.join("unresponsive-codex");
    fs::write(
        &binary,
        r#"#!/bin/sh
echo $$ > child.pid
sleep 300 &
echo $! > grandchild.pid
while IFS= read -r line; do
  case "$line" in
    *'"id":0'*) printf '%s\n' '{"id":0,"result":{}}' ;;
    *'"id":1'*) printf '%s\n' '{"id":1,"result":{"thread":{"id":"thread-force"}}}' ;;
    *'"id":2'*) printf '%s\n' '{"id":2,"result":{"turn":{"id":"turn-force"}}}' ;;
    *'"id":3'*) : > interrupt-seen ;;
  esac
done
"#,
    )
    .unwrap();
    fs::set_permissions(&binary, fs::Permissions::from_mode(0o700)).unwrap();
    let adapter = CodexAppServerAdapter::new(CodexAppServerConfig {
        codex_binary: binary,
        interrupt_grace_period: Duration::from_millis(50),
        ..CodexAppServerConfig::default()
    })
    .unwrap();
    let cancellation = CancellationToken::new();
    let mut stream = adapter
        .run(AgentRunRequest {
            request_id: "force-after-timeout".into(),
            model: None,
            reasoning_effort: None,
            project_instructions: None,
            prompt: "hello".into(),
            cwd: cwd.clone(),
            resume_thread_id: None,
            sandbox: SandboxMode::ReadOnly,
            approval_policy: ApprovalPolicy::Never,
            output_schema: None,
            cancellation: cancellation.clone(),
        })
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while !matches!(
            stream.next().await,
            Some(Ok(AgentEvent::TurnStarted { .. }))
        ) {}
    })
    .await
    .expect("fake app-server must start a turn");
    let (pid, grandchild_pid) = read_owned_pids(&cwd).await;

    let started = tokio::time::Instant::now();
    cancellation.cancel();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match stream.next().await {
                Some(Err(error)) => {
                    assert_eq!(error.kind, AdapterErrorKind::Cancelled);
                    break;
                }
                Some(Ok(_)) => {}
                None => panic!("stream closed without cancellation result"),
            }
        }
        assert!(stream.next().await.is_none());
    })
    .await
    .expect("interrupt timeout must force cleanup and settle the stream");
    assert!(started.elapsed() >= Duration::from_millis(40));
    assert!(cwd.join("interrupt-seen").exists());
    assert_dead(pid.trim()).await;
    assert_dead(grandchild_pid.trim()).await;
}

async fn read_owned_pids(cwd: &std::path::Path) -> (String, String) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let (Ok(pid), Ok(grandchild_pid)) = (
                fs::read_to_string(cwd.join("child.pid")),
                fs::read_to_string(cwd.join("grandchild.pid")),
            ) && !pid.trim().is_empty()
                && !grandchild_pid.trim().is_empty()
            {
                break (pid, grandchild_pid);
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap()
}

async fn assert_dead(pid: &str) {
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let alive = std::process::Command::new("kill")
                .args(["-0", pid])
                .output()
                .unwrap();
            if !alive.status.success() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the adapter must reclaim its entire owned process group");
}
