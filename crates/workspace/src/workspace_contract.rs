//! Reusable contract assertions for `ProjectWorkspace` adapter implementations.

use std::path::{Path, PathBuf};

use ait_domain::ErrorCode;

use crate::ProjectWorkspace;

/// Exercise two independent adapter instances against two empty local directories.
///
/// Callers own fixture allocation and cleanup. No storage/provider adapter is used.
///
/// # Panics
///
/// Panics if preparation, baseline identity, exclusion, or resource release violates
/// the [`ProjectWorkspace`] contract.
pub async fn assert_workspace_contract(
    first: &dyn ProjectWorkspace,
    independent: &dyn ProjectWorkspace,
    root: &Path,
    other: &Path,
) {
    let (root, other, head) = assert_git_contract(first, root, other).await;
    assert_lease_and_worktree_contract(first, independent, &root, &other, &head).await;
}

async fn assert_git_contract(
    workspace: &dyn ProjectWorkspace,
    root: &Path,
    other: &Path,
) -> (PathBuf, PathBuf, String) {
    let root = workspace
        .prepare_git_root(root, None)
        .await
        .expect("first workspace root must be prepared");
    let other = workspace
        .prepare_git_root(other, None)
        .await
        .expect("second workspace root must be prepared");
    workspace
        .verify_git_root(&root)
        .await
        .expect("prepared workspace root must verify");
    assert_eq!(
        workspace
            .prepare_git_root(&root, Some(&root))
            .await
            .expect("unchanged workspace root must be reusable"),
        root
    );
    assert_eq!(
        workspace
            .git_head(&root)
            .await
            .expect("unborn HEAD must be observable"),
        None
    );
    assert_eq!(
        workspace
            .clean_baseline(&root)
            .await
            .expect_err("unborn repository must not have a clean baseline")
            .code,
        ErrorCode::ProjectGitHeadUnavailable
    );
    let head = workspace
        .ensure_git_head(&root)
        .await
        .expect("first workspace HEAD must be initialized");
    workspace
        .ensure_git_head(&other)
        .await
        .expect("second workspace HEAD must be initialized");
    assert_eq!(
        workspace
            .ensure_git_head(&root)
            .await
            .expect("initialized HEAD must be stable"),
        head
    );
    assert_eq!(
        workspace
            .clean_baseline(&root)
            .await
            .expect("initialized workspace must have a clean baseline")
            .commit,
        head
    );
    assert!(
        workspace
            .symbolic_head(&root)
            .await
            .expect("symbolic HEAD must be observable")
            .is_some()
    );
    assert!(
        workspace
            .git_dir(&root)
            .await
            .expect("Git directory must be observable")
            .is_absolute()
    );
    (root, other, head)
}

async fn assert_lease_and_worktree_contract(
    first: &dyn ProjectWorkspace,
    independent: &dyn ProjectWorkspace,
    root: &Path,
    other: &Path,
    head: &str,
) {
    let lease = first
        .acquire_lease(root)
        .await
        .expect("first workspace lease must be acquired");
    assert_eq!(lease.canonical_root(), root);
    let Err(conflict) = independent.acquire_lease(root).await else {
        panic!("independent adapter must observe the held lease");
    };
    assert_eq!(conflict.code, ErrorCode::ProjectWorkspaceBusy);
    let parallel = independent
        .acquire_lease(other)
        .await
        .expect("unrelated workspace lease must be independent");
    let clone = lease.clone();
    drop(lease);
    assert!(independent.acquire_lease(root).await.is_err());
    drop(clone);
    let reacquired = independent
        .acquire_lease(root)
        .await
        .expect("workspace lease must be available after final drop");
    let worktree = root.join(".ait").join("contract-session");
    assert!(
        independent
            .ensure_session_worktree(root, &worktree, head, Some(reacquired.clone()))
            .await
            .expect("new Session worktree must be created")
    );
    assert!(
        !independent
            .ensure_session_worktree(root, &worktree, head, Some(reacquired))
            .await
            .expect("existing Session worktree must be reused")
    );
    assert_eq!(
        first
            .clean_baseline(&worktree)
            .await
            .expect("Session worktree must have a clean baseline")
            .commit,
        head
    );
    assert_eq!(
        first
            .symbolic_head(&worktree)
            .await
            .expect("Session worktree HEAD must be observable"),
        None
    );
    assert_eq!(
        first
            .clean_baseline(root)
            .await
            .expect("primary workspace baseline must remain clean")
            .commit,
        head
    );
    drop(parallel);
}
