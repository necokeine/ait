use super::*;
use crate::LocalProjectWorkspace;
use ait_workspace::ProjectWorkspace;
use std::{
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};

fn git(root: &Path, args: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn head_and_index_races_after_status_are_rejected() {
    for move_head in [true, false] {
        let root = tempfile::tempdir().unwrap();
        let mut context = BlockingContext {
            cancellation: CancellationToken::new(),
            deadline: Instant::now() + DEADLINE,
            failure_code: ErrorCode::ProjectGitHeadUnavailable,
            options: OperationOptions::default(),
            retained: Mutex::default(),
            after_git: None,
        };
        context.prepare_git_root(root.path(), None).unwrap();
        context.ensure_git_head(root.path()).unwrap();
        let path = root.path().to_owned();
        let changed = AtomicBool::new(false);
        context.after_git = Some(Box::new(move |command| {
            if command.get_args().any(|arg| arg == "status")
                && !changed.swap(true, Ordering::SeqCst)
            {
                if move_head {
                    git(
                        &path,
                        &[
                            "-c",
                            "user.name=Fixture",
                            "-c",
                            "user.email=fixture@localhost",
                            "commit",
                            "--allow-empty",
                            "-m",
                            "concurrent",
                        ],
                    );
                } else {
                    std::fs::write(path.join("staged"), "concurrent").unwrap();
                    git(&path, &["add", "staged"]);
                }
            }
        }));
        let failure = context.clean_git_baseline(root.path()).unwrap_err();
        assert_eq!(
            failure.code,
            if move_head {
                ErrorCode::ProjectGitHeadUnavailable
            } else {
                ErrorCode::ProjectGitDirty
            }
        );
        assert!(failure.retryable);
    }
}

#[cfg(unix)]
#[test]
fn deadline_kills_and_reaps_a_stalled_git_process_group() {
    let context = BlockingContext {
        cancellation: CancellationToken::new(),
        deadline: Instant::now() + Duration::from_millis(80),
        failure_code: ErrorCode::ProjectGitHeadUnavailable,
        options: OperationOptions::default(),
        retained: Mutex::default(),
        after_git: None,
    };
    let start = Instant::now();
    // Git aliases exercise a real Git child and its shell descendant.
    let result = context
        .command()
        .args(["-c", "alias.stall=!sleep 20", "stall"])
        .output();
    assert!(result.is_err());
    assert!(start.elapsed() < Duration::from_secs(2));
    assert!(context.check().unwrap_err().retryable);
}

#[tokio::test]
async fn dropped_future_retains_the_lease_until_started_blocking_io_drains() {
    let root = tempfile::tempdir().unwrap();
    let adapter = LocalProjectWorkspace::default();
    adapter.prepare_git_root(root.path(), None).await.unwrap();
    adapter.ensure_git_head(root.path()).await.unwrap();
    let lease = adapter.acquire_lease(root.path()).await.unwrap();
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let (finished_tx, finished_rx) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(async move {
        let operation = Operation::new(
            &OperationOptions::default(),
            ErrorCode::ProjectGitInitFailed,
        );
        operation
            .run(move |context| {
                let held_lease = lease;
                started_tx.send(()).unwrap();
                release_rx.recv().unwrap();
                let cancelled = context.check().unwrap_err().code == ErrorCode::RunCancelled;
                drop(held_lease);
                finished_tx.send(cancelled).unwrap();
                Ok(())
            })
            .await
    });
    started_rx.await.unwrap();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    let contender = LocalProjectWorkspace::default();
    assert_eq!(
        contender
            .acquire_lease(root.path())
            .await
            .err()
            .unwrap()
            .code,
        ErrorCode::ProjectWorkspaceBusy
    );
    release_tx.send(()).unwrap();
    assert!(finished_rx.await.unwrap());
    let _next = contender.acquire_lease(root.path()).await.unwrap();
}
