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
        .env("CODEX_THREAD_ID", "unrelated-thread")
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
async fn cancellation_during_resume_handshake_preserves_preexisting_same_thread_processes() {
    let directory = tempfile::tempdir().unwrap();
    let cwd = directory.path().canonicalize().unwrap();
    let binary = cwd.join("stalled-resume-codex");
    fs::write(
        &binary,
        r#"#!/bin/sh
echo $$ > child.pid
sleep 300 &
echo $! > grandchild.pid
while IFS= read -r line; do
  case "$line" in
    *'"id":0'*) printf '%s\n' '{"id":0,"result":{}}' ;;
    *'"id":-1'*) printf '%s\n' '{"id":-1,"result":{"config":{"shell_environment_policy":{"inherit":"none","include_only":["PATH"]}},"origins":{}}}' ;;
    *'"id":1'*)
      printf '%s' "$line" | "$(command -v python3)" -c 'import json,sys; request=json.load(sys.stdin); config=request["params"]["config"]; assert request["method"] == "thread/resume"; assert request["params"]["threadId"] == "existing-thread"; assert config["shell_environment_policy.include_only"] == ["PATH", "AIT_CODEX_PROCESS_OWNER"]; assert config["shell_environment_policy.set.AIT_CODEX_PROCESS_OWNER"]'
      : > resume-seen
      ;;
  esac
done
"#,
    )
    .unwrap();
    fs::set_permissions(&binary, fs::Permissions::from_mode(0o700)).unwrap();
    let adapter = CodexAppServerAdapter::new(CodexAppServerConfig {
        codex_binary: binary,
        ..CodexAppServerConfig::default()
    })
    .unwrap();
    let mut same_thread_preexisting = std::process::Command::new("sleep")
        .arg("300")
        .env("CODEX_THREAD_ID", "existing-thread")
        .spawn()
        .unwrap();
    let cancellation = CancellationToken::new();
    let mut stream = adapter
        .run(AgentRunRequest {
            request_id: "cancel-resume-handshake".into(),
            model: None,
            reasoning_effort: None,
            project_instructions: None,
            prompt: "hello".into(),
            cwd: cwd.clone(),
            resume_thread_id: Some("existing-thread".into()),
            sandbox: SandboxMode::ReadOnly,
            approval_policy: ApprovalPolicy::Never,
            output_schema: None,
            cancellation: cancellation.clone(),
        })
        .await
        .unwrap();
    let root_pid = read_pid(&cwd, "child.pid").await;
    let grandchild_pid = read_pid(&cwd, "grandchild.pid").await;
    tokio::time::timeout(Duration::from_secs(5), async {
        while !cwd.join("resume-seen").exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("fake app-server must receive the resume ownership override");

    cancellation.cancel();
    tokio::time::timeout(Duration::from_secs(5), async {
        let error = stream.next().await.unwrap().unwrap_err();
        assert_eq!(error.kind, AdapterErrorKind::Cancelled);
        assert!(stream.next().await.is_none());
    })
    .await
    .expect("resume handshake cancellation must settle");
    assert_dead(root_pid.trim()).await;
    assert_dead(grandchild_pid.trim()).await;
    assert!(
        same_thread_preexisting.try_wait().unwrap().is_none(),
        "per-Run cleanup must not kill a process that predates this invocation"
    );
    same_thread_preexisting.kill().unwrap();
    same_thread_preexisting.wait().unwrap();
}

#[tokio::test]
async fn missing_interrupt_terminal_forces_the_owned_process_tree_to_exit() {
    let directory = tempfile::tempdir().unwrap();
    let cwd = directory.path().canonicalize().unwrap();
    let binary = cwd.join("unresponsive-codex");
    let mut same_thread_preexisting = std::process::Command::new("sleep")
        .arg("300")
        .env("CODEX_THREAD_ID", "thread-force")
        .spawn()
        .unwrap();
    fs::write(
        &binary,
        r#"#!/bin/sh
echo $$ > child.pid
while IFS= read -r line; do
  case "$line" in
    *'"id":0'*) printf '%s\n' '{"id":0,"result":{}}' ;;
    *'"id":-1'*) printf '%s\n' '{"id":-1,"result":{"config":{"shell_environment_policy":{"inherit":"none","include_only":["PATH"]}},"origins":{}}}' ;;
    *'"id":1'*)
      printf '%s' "$line" | "$(command -v python3)" -c 'import json,sys,pathlib; config=json.load(sys.stdin)["params"]["config"]; assert config["shell_environment_policy.include_only"] == ["PATH", "AIT_CODEX_PROCESS_OWNER"]; pathlib.Path("owner-marker").write_text(config["shell_environment_policy.set.AIT_CODEX_PROCESS_OWNER"])'
      printf '%s\n' '{"id":1,"result":{"thread":{"id":"thread-force"}}}'
      ;;
    *'"id":2'*)
      owner_marker=$(cat owner-marker)
      /usr/bin/env -i CODEX_THREAD_ID=thread-force AIT_CODEX_PROCESS_OWNER="$owner_marker" "$(command -v python3)" -c 'import os,time,pathlib; assert os.environ["AIT_CODEX_PROCESS_OWNER"] == pathlib.Path("owner-marker").read_text(); assert os.environ["CODEX_THREAD_ID"] == "thread-force"; os.setsid(); pathlib.Path("detached.pid").write_text(str(os.getpid())); child=os.fork(); pathlib.Path("detached-grandchild.pid").write_text(str(os.getpid())) if child == 0 else None; time.sleep(300)' </dev/null >/dev/null 2>&1 &
      printf '%s\n' '{"id":2,"result":{"turn":{"id":"turn-force"}}}'
      ;;
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
    let root_pid = read_pid(&cwd, "child.pid").await;
    let detached_pid = read_pid(&cwd, "detached.pid").await;
    let detached_grandchild_pid = read_pid(&cwd, "detached-grandchild.pid").await;
    assert_ne!(
        process_group(detached_pid.trim()),
        process_group(root_pid.trim()),
        "the fake tool must use setsid() like Codex shell tools"
    );

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
    assert_dead(root_pid.trim()).await;
    assert_dead(detached_pid.trim()).await;
    assert_dead(detached_grandchild_pid.trim()).await;
    assert!(
        same_thread_preexisting.try_wait().unwrap().is_none(),
        "per-Run cleanup must not kill a pre-existing process from the same thread"
    );
    same_thread_preexisting.kill().unwrap();
    same_thread_preexisting.wait().unwrap();
}

#[tokio::test]
#[allow(
    clippy::too_many_lines,
    reason = "the production-wiring fixture keeps its process choreography visible"
)]
async fn filtered_tool_environment_does_not_orphan_thread_descendants() {
    let directory = tempfile::tempdir().unwrap();
    let cwd = directory.path().canonicalize().unwrap();
    let binary = cwd.join("exiting-codex");
    fs::write(
        cwd.join("forking-tool.py"),
        r#"import os
import pathlib
import time

assert os.environ["AIT_CODEX_PROCESS_OWNER"] == pathlib.Path("owner-marker").read_text()
assert os.environ["CODEX_THREAD_ID"] == "thread-root-exit"
os.setsid()

def record(pid):
    fd = os.open("forked-pids", os.O_CREAT | os.O_APPEND | os.O_WRONLY, 0o600)
    try:
        os.write(fd, f"{pid}\n".encode())
    finally:
        os.close(fd)

record(os.getpid())
while not pathlib.Path("fork-now").exists():
    time.sleep(0.001)

for index in range(40):
    child = os.fork()
    if child == 0:
        grandchild = os.fork()
        if grandchild == 0:
            record(os.getpid())
            time.sleep(300)
        record(os.getpid())
        time.sleep(0.02)
        os._exit(0)
    record(child)
    if index == 0:
        pathlib.Path("first-forked").touch()
    time.sleep(0.005)

time.sleep(300)
"#,
    )
    .unwrap();
    fs::write(
        &binary,
        r#"#!/bin/sh
echo $$ > child.pid
while IFS= read -r line; do
  case "$line" in
    *'"id":0'*) printf '%s\n' '{"id":0,"result":{}}' ;;
    *'"id":-1'*) printf '%s\n' '{"id":-1,"result":{"config":{"shell_environment_policy":{"inherit":"core","include_only":["PATH"]}},"origins":{}}}' ;;
    *'"id":1'*)
      printf '%s' "$line" | "$(command -v python3)" -c 'import json,sys,pathlib; config=json.load(sys.stdin)["params"]["config"]; assert config["shell_environment_policy.include_only"] == ["PATH", "AIT_CODEX_PROCESS_OWNER"]; pathlib.Path("owner-marker").write_text(config["shell_environment_policy.set.AIT_CODEX_PROCESS_OWNER"])'
      printf '%s\n' '{"id":1,"result":{"thread":{"id":"thread-root-exit"}}}'
      ;;
    *'"id":2'*)
      owner_marker=$(cat owner-marker)
      /usr/bin/env -i CODEX_THREAD_ID=thread-root-exit AIT_CODEX_PROCESS_OWNER="$owner_marker" "$(command -v python3)" forking-tool.py </dev/null >/dev/null 2>&1 &
      printf '%s\n' '{"id":2,"result":{"turn":{"id":"turn-root-exit"}}}'
      ;;
    *'"id":3'*)
      : > interrupt-seen
      : > fork-now
      while [ ! -f first-forked ]; do sleep 0.001; done
      exit 0
      ;;
  esac
done
"#,
    )
    .unwrap();
    fs::set_permissions(&binary, fs::Permissions::from_mode(0o700)).unwrap();
    let adapter = CodexAppServerAdapter::new(CodexAppServerConfig {
        codex_binary: binary,
        interrupt_grace_period: Duration::from_millis(200),
        ..CodexAppServerConfig::default()
    })
    .unwrap();
    let cancellation = CancellationToken::new();
    let mut stream = adapter
        .run(AgentRunRequest {
            request_id: "root-exit-fork-race".into(),
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
    let root_pid = read_pid(&cwd, "child.pid").await;
    let mut unrelated = std::process::Command::new("sleep")
        .arg("300")
        .env("CODEX_THREAD_ID", "unrelated-thread")
        .spawn()
        .unwrap();

    cancellation.cancel();
    tokio::time::timeout(Duration::from_secs(8), async {
        while stream.next().await.is_some() {}
    })
    .await
    .expect("root exit and concurrent descendant forking must still settle");

    assert!(cwd.join("interrupt-seen").exists());
    assert_dead(root_pid.trim()).await;
    let forked = fs::read_to_string(cwd.join("forked-pids")).unwrap();
    let forked = forked.lines().collect::<std::collections::HashSet<_>>();
    assert!(
        forked.len() >= 3,
        "fixture must create a detached process tree"
    );
    for pid in forked {
        assert_dead(pid).await;
    }
    assert!(
        unrelated.try_wait().unwrap().is_none(),
        "thread-owned cleanup must not kill an unrelated process"
    );
    unrelated.kill().unwrap();
    unrelated.wait().unwrap();
}

async fn read_pid(cwd: &std::path::Path, filename: &str) -> String {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Ok(pid) = fs::read_to_string(cwd.join(filename))
                && !pid.trim().is_empty()
            {
                break pid;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap()
}

fn process_group(pid: &str) -> String {
    let output = std::process::Command::new("ps")
        .args(["-o", "pgid=", "-p", pid])
        .output()
        .unwrap();
    assert!(output.status.success());
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

async fn assert_dead(pid: &str) {
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let state = std::process::Command::new("ps")
                .args(["-o", "state=", "-p", pid])
                .output()
                .unwrap();
            if !state.status.success()
                || String::from_utf8_lossy(&state.stdout)
                    .trim()
                    .starts_with('Z')
                || state.stdout.is_empty()
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the adapter must reclaim every tracked descendant");
}
