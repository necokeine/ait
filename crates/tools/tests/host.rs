//! Filesystem confinement, bounded output and cancellation of the actual host slice.
#![allow(clippy::pedantic)]
use ait_domain::{ErrorCode, RunId, RunPermissionProfile, SandboxAccess, ToolExecutionId};
use ait_ports::{RunToolFactory, ToolInvocation};
use ait_tools::host::{HostIoCheckpoint, HostIoObserver};
use ait_tools::host::{HostToolFactory, MAX_BYTES};
use serde_json::{Value, json};
use std::sync::{Arc, Condvar, Mutex};
use tokio_util::sync::CancellationToken;

#[derive(Default)]
struct Overlap {
    entered: Mutex<usize>,
    wake: Condvar,
    overlapped: std::sync::atomic::AtomicBool,
}
impl HostIoObserver for Overlap {
    fn checkpoint(&self, _: &str, point: HostIoCheckpoint) {
        if point != HostIoCheckpoint::BeforeRead {
            return;
        }
        let mut count = self.entered.lock().unwrap();
        *count += 1;
        if *count == 2 {
            self.overlapped
                .store(true, std::sync::atomic::Ordering::SeqCst);
            self.wake.notify_all();
        }
        let (mut count, _) = self
            .wake
            .wait_timeout_while(count, std::time::Duration::from_secs(1), |_| {
                !self.overlapped.load(std::sync::atomic::Ordering::SeqCst)
            })
            .unwrap();
        *count -= 1;
    }
}
#[tokio::test]
async fn production_file_reads_overlap_without_blocking_the_async_runtime() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("file"), "actual file").unwrap();
    let overlap = Arc::new(Overlap::default());
    let tools = HostToolFactory::with_observer(overlap.clone())
        .create(
            &root.path().canonicalize().unwrap(),
            RunPermissionProfile::default(),
        )
        .unwrap();
    let (first, second) = tokio::join!(
        tools.execute(call("read", json!({"file_path":"file"}))),
        tools.execute(call("read", json!({"file_path":"file"})))
    );
    assert_eq!(first.unwrap().output["text"], "1: actual file\n");
    assert_eq!(second.unwrap().output["text"], "1: actual file\n");
    assert!(
        overlap.overlapped.load(std::sync::atomic::Ordering::SeqCst),
        "real HostTools must have two active file workers"
    );
}
fn call(name: &str, args: Value) -> ToolInvocation {
    ToolInvocation {
        run_id: RunId::new("run"),
        call_id: "call".into(),
        execution_id: ToolExecutionId::new("execution"),
        tool_name: name.into(),
        arguments: args,
        cancellation: CancellationToken::new(),
    }
}
#[tokio::test]
async fn fixed_sandbox_blocks_writes_escape_symlinks_and_preserves_atomic_edits() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("file"), "old\n").unwrap();
    let read = HostToolFactory
        .create(
            &root.path().canonicalize().unwrap(),
            RunPermissionProfile::default(),
        )
        .unwrap();
    assert!(
        read.execute(call("write", json!({"file_path":"file","content":"bad"})))
            .await
            .is_err()
    );
    for mode in [SandboxAccess::WorkspaceWrite, SandboxAccess::FullAccess] {
        let tools = HostToolFactory
            .create(
                &root.path().canonicalize().unwrap(),
                RunPermissionProfile {
                    sandbox: mode,
                    ..Default::default()
                },
            )
            .unwrap();
        for path in [
            "../outside",
            ".git/config",
            ".ait/database",
            ".env",
            outside.path().join("absolute").to_str().unwrap(),
        ] {
            assert!(
                tools
                    .execute(call("write", json!({"file_path":path,"content":"bad"})))
                    .await
                    .is_err()
            );
        }
        assert!(tools.execute(call("write",json!({"file_path":"file","content":"bad","sandbox_permissions":"danger-full-access"}))).await.is_err());
        assert!(
            tools
                .execute(call(
                    "write",
                    json!({"file_path":"large","content":"x".repeat(MAX_BYTES+1)})
                ))
                .await
                .is_err()
        );
        tools
            .execute(call(
                "edit",
                json!({"file_path":"file","old_string":"old","new_string":"new"}),
            ))
            .await
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(root.path().join("file")).unwrap(),
            "new\n"
        );
        std::fs::write(root.path().join("file"), "old\n").unwrap();
        let repetitive = "x".repeat(MAX_BYTES);
        std::fs::write(root.path().join("expansion"), &repetitive).unwrap();
        assert!(
            tools
                .execute(call(
                    "edit",
                    json!({"file_path":"expansion","old_string":"x","new_string":"y".repeat(1024),"replace_all":true})
                ))
                .await
                .is_err()
        );
        assert_eq!(
            std::fs::read_to_string(root.path().join("expansion")).unwrap(),
            repetitive
        );
    }
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(outside.path(), root.path().join("link")).unwrap();
        let tools = HostToolFactory
            .create(
                &root.path().canonicalize().unwrap(),
                RunPermissionProfile {
                    sandbox: SandboxAccess::WorkspaceWrite,
                    ..Default::default()
                },
            )
            .unwrap();
        assert!(
            tools
                .execute(call(
                    "write",
                    json!({"file_path":"link/escape","content":"bad"})
                ))
                .await
                .is_err()
        );
        assert!(!outside.path().join("escape").exists());
        std::fs::write(outside.path().join("secret"), "private").unwrap();
        std::fs::hard_link(outside.path().join("secret"), root.path().join("hard")).unwrap();
        tools
            .execute(call("write", json!({"file_path":"hard","content":"new"})))
            .await
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(outside.path().join("secret")).unwrap(),
            "private"
        );
    }
}
#[cfg(unix)]
#[tokio::test]
async fn shell_is_controlled_cancellable_and_bounded() {
    let root = tempfile::tempdir().unwrap();
    let tools = HostToolFactory
        .create(
            &root.path().canonicalize().unwrap(),
            RunPermissionProfile::default(),
        )
        .unwrap();
    let good = tools
        .execute(call(
            "bash",
            json!({"command":"printf hello","description":"Print a greeting"}),
        ))
        .await
        .unwrap();
    assert_eq!(good.output["stdout"], "hello");
    for command in [
        "sh -c 'touch bad'",
        "env",
        "cat /etc/passwd",
        "echo hi > bad",
    ] {
        let result = tools
            .execute(call(
                "bash",
                json!({"command":command,"description":"Try command"}),
            ))
            .await;
        if command.starts_with("echo") {
            assert!(result.is_ok());
            assert!(!root.path().join("bad").exists());
        } else {
            assert!(result.is_err());
        }
    }
    let timeout = tools
        .execute(call(
            "bash",
            json!({"command":"sleep 1","description":"Wait","timeoutMs":1}),
        ))
        .await
        .unwrap_err();
    assert_eq!(timeout.code, ErrorCode::RunLimitExceeded);
    let output = tools
        .execute(call(
            "bash",
            json!({"command":"printf %100000s x","description":"Large output"}),
        ))
        .await
        .unwrap_err();
    assert_eq!(output.code, ErrorCode::ToolExecutionFailed);
    let request = call("bash", json!({"command":"sleep 30","description":"Wait"}));
    let cancellation = request.cancellation.clone();
    let work = tools.execute(request);
    let cancel = async {
        tokio::task::yield_now().await;
        cancellation.cancel();
    };
    let (result, ()) = tokio::join!(work, cancel);
    assert_eq!(result.unwrap_err().code, ErrorCode::RunCancelled);
}
