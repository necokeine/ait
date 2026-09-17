//! One-operation authority checks at the real filesystem and OS execution boundaries.
#![cfg(unix)]
#![allow(clippy::pedantic)]
use ait_domain::*;
use ait_ports::{RunToolFactory, ToolInvocation};
use ait_tools::host::{HostIoCheckpoint, HostIoObserver, HostToolFactory};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::sync::Arc;

fn execution(name: &str, arguments: Value) -> ToolExecution {
    ToolExecution {
        id: ToolExecutionId::new("execution"),
        run_id: RunId::new("run"),
        call_id: "call".into(),
        assistant_message_id: MessageId::from_u128(1),
        tool_use_index: 0,
        tool_result_message_id: None,
        tool_name: name.into(),
        arguments,
        attempt: 1,
        approval_status: ToolApprovalStatus::Pending,
        status: ToolExecutionStatus::Pending,
        result: None,
        error: None,
        started_at: None,
        ended_at: None,
        created_at: TimestampMs(1),
    }
}
fn call(e: &ToolExecution) -> ToolInvocation {
    ToolInvocation {
        run_id: e.run_id.clone(),
        call_id: e.call_id.clone(),
        execution_id: e.id.clone(),
        tool_name: e.tool_name.clone(),
        arguments: e.arguments.clone(),
        usage: Default::default(),
        cancellation: tokio_util::sync::CancellationToken::new(),
    }
}
fn grant(root: &std::path::Path, e: &ToolExecution) -> ToolGrant {
    ToolGrant {
        request_id: "approval".into(),
        run_id: "run".into(),
        execution_id: "execution".into(),
        call_id: "call".into(),
        arguments_digest: format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(&e.arguments).unwrap())
        ),
        target: HostToolFactory
            .review(root, RunPermissionProfile::default(), e)
            .unwrap()
            .unwrap(),
        lease_epoch: 1,
        expires_at: i64::MAX,
    }
}

#[test]
fn unreviewable_file_targets_never_offer_authority() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    std::fs::create_dir(root.join("folder")).unwrap();
    for path in [
        "",
        ".",
        "folder",
        "../outside",
        ".hidden",
        "/absolute",
        "missing/file",
    ] {
        let e = execution("write", json!({"file_path":path,"content":"synthetic"}));
        assert!(
            HostToolFactory
                .review(&root, RunPermissionProfile::default(), &e)
                .is_err(),
            "{path}"
        );
    }
}

#[tokio::test]
async fn grant_binding_rejects_modified_arguments_run_call_execution_expiry_and_paths() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let e = execution(
        "write",
        json!({"file_path":"target.txt","content":"approved"}),
    );
    let g = grant(&root, &e);
    let tools = HostToolFactory
        .create(&root, RunPermissionProfile::default())
        .unwrap();
    for change in ["args", "run", "call", "execution", "expired"] {
        let mut invocation = call(&e);
        let mut authority = g.clone();
        match change {
            "args" => invocation.arguments["content"] = json!("unapproved"),
            "run" => invocation.run_id = RunId::new("another"),
            "call" => invocation.call_id = "another".into(),
            "execution" => invocation.execution_id = ToolExecutionId::new("another"),
            _ => authority.expires_at = 0,
        }
        assert!(tools.execute_granted(invocation, authority).await.is_err());
        assert!(!root.join("target.txt").exists());
    }
    let outside = tempfile::tempdir().unwrap();
    let target = outside.path().join("synthetic.txt");
    std::fs::write(&target, "unchanged").unwrap();
    std::os::unix::fs::symlink(&target, root.join("target.txt")).unwrap();
    assert!(tools.execute_granted(call(&e), g).await.is_err());
    assert_eq!(std::fs::read_to_string(target).unwrap(), "unchanged");
}

#[tokio::test]
async fn one_shell_grant_cannot_execute_twice_or_change_the_baseline() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let e = execution(
        "bash",
        json!({
            "command": "printf once >> count.txt",
            "description": "Append a synthetic marker",
            "sandbox_permissions": "workspace-write",
        }),
    );
    // Backend unavailability is an explicit failure, never a skipped sandbox pass.
    let g = grant(&root, &e);
    let tools = HostToolFactory
        .create(&root, RunPermissionProfile::default())
        .unwrap();
    assert_eq!(
        tools
            .execute_granted(call(&e), g.clone())
            .await
            .unwrap()
            .output["exit_status"],
        0
    );
    assert!(tools.execute_granted(call(&e), g).await.is_err());
    assert!(tools.execute(call(&e)).await.is_err());
    assert_eq!(
        std::fs::read_to_string(root.join("count.txt")).unwrap(),
        "once"
    );
}

struct SwapParent(std::path::PathBuf);
impl HostIoObserver for SwapParent {
    fn checkpoint(&self, _: &str, point: HostIoCheckpoint) {
        if point == HostIoCheckpoint::BeforePublish {
            std::fs::rename(self.0.join("parent"), self.0.join("old-parent")).unwrap();
            std::fs::create_dir(self.0.join("parent")).unwrap();
        }
    }
}

struct ExpireAtPublish;
impl HostIoObserver for ExpireAtPublish {
    fn checkpoint(&self, _: &str, point: HostIoCheckpoint) {
        if point == HostIoCheckpoint::BeforePublish {
            std::thread::sleep(std::time::Duration::from_millis(600));
        }
    }
}

#[tokio::test]
async fn grant_expiry_is_rechecked_at_the_actual_publication_boundary() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let e = execution(
        "write",
        json!({"file_path":"effect.txt","content":"must not publish"}),
    );
    let tools = HostToolFactory::with_observer(Arc::new(ExpireAtPublish))
        .create(&root, RunPermissionProfile::default())
        .unwrap();
    let mut g = grant(&root, &e);
    g.expires_at = i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis(),
    )
    .unwrap()
        + 500;
    assert!(tools.execute_granted(call(&e), g).await.is_err());
    assert!(!root.join("effect.txt").exists());
}
#[tokio::test]
async fn changed_parent_at_the_publication_boundary_invalidates_grant() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    std::fs::create_dir(root.join("parent")).unwrap();
    let e = execution(
        "write",
        json!({"file_path":"parent/file","content":"must not publish"}),
    );
    let g = grant(&root, &e);
    let tools = HostToolFactory::with_observer(Arc::new(SwapParent(root.clone())))
        .create(&root, RunPermissionProfile::default())
        .unwrap();
    assert!(tools.execute_granted(call(&e), g).await.is_err());
    assert!(!root.join("parent/file").exists());
    assert!(!root.join("old-parent/file").exists());
}
