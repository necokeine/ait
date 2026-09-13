//! Reusable contract assertions for `ProjectWorkspace` adapter implementations.
use crate::ProjectWorkspace;
use ait_domain::ErrorCode;
use std::path::Path;

/// Exercise two independent adapter instances against two empty local directories.
/// Callers own fixture allocation and cleanup. No storage/provider adapter is used.
/// # Panics
/// Panics if preparation, baseline identity, exclusion, or resource release violates
/// the `ProjectWorkspace` contract.
pub async fn assert_workspace_contract(
    first: &dyn ProjectWorkspace,
    independent: &dyn ProjectWorkspace,
    root: &Path,
    other: &Path,
) {
    let root = first.prepare_git_root(root).await.unwrap();
    let other = first.prepare_git_root(other).await.unwrap();
    assert_eq!(first.git_head(&root).await.unwrap(), None);
    assert_eq!(
        first.clean_baseline(&root).await.unwrap_err().code,
        ErrorCode::ProjectGitHeadUnavailable
    );
    let head = first.ensure_git_head(&root).await.unwrap();
    first.ensure_git_head(&other).await.unwrap();
    assert_eq!(first.ensure_git_head(&root).await.unwrap(), head);
    assert_eq!(first.clean_baseline(&root).await.unwrap().commit, head);
    assert!(first.symbolic_head(&root).await.unwrap().is_some());
    assert!(first.git_dir(&root).await.unwrap().is_absolute());
    let lease = first.acquire_lease(&root).await.unwrap();
    assert_eq!(lease.canonical_root(), root);
    assert_eq!(
        independent.acquire_lease(&root).await.err().unwrap().code,
        ErrorCode::ProjectWorkspaceBusy
    );
    let parallel = independent.acquire_lease(&other).await.unwrap();
    let clone = lease.clone();
    drop(lease);
    assert!(independent.acquire_lease(&root).await.is_err());
    drop(clone);
    let reacquired = independent.acquire_lease(&root).await.unwrap();
    let worktree = root.join(".ait").join("contract-session");
    assert!(
        independent
            .ensure_session_worktree(&root, &worktree, &head, Some(reacquired.clone()))
            .await
            .unwrap()
    );
    assert!(
        !independent
            .ensure_session_worktree(&root, &worktree, &head, Some(reacquired))
            .await
            .unwrap()
    );
    assert_eq!(first.clean_baseline(&worktree).await.unwrap().commit, head);
    assert_eq!(first.symbolic_head(&worktree).await.unwrap(), None);
    assert_eq!(first.clean_baseline(&root).await.unwrap().commit, head);
    drop(parallel);
}
