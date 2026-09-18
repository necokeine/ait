//! Real Git/OS coverage for the ProjectWorkspace port.
#![allow(clippy::pedantic)]
use ait_domain::ErrorCode;
use ait_workspace::{ProjectWorkspace, workspace_contract};
use ait_workspace_local::LocalProjectWorkspace;
use std::{path::Path, process::Command, sync::Arc};

fn git(root: &Path, args: &[&str]) {
    let result = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
}

#[tokio::test]
async fn shared_port_contract() {
    let root = tempfile::tempdir().unwrap();
    let other = tempfile::tempdir().unwrap();
    workspace_contract::assert_workspace_contract(
        &LocalProjectWorkspace::default(),
        &LocalProjectWorkspace::default(),
        root.path(),
        other.path(),
    )
    .await;
}

#[tokio::test]
async fn baseline_revalidation_never_refreshes_or_rewrites_the_index() {
    let temp = tempfile::tempdir().unwrap();
    let adapter = LocalProjectWorkspace::default();
    let root = adapter.prepare_git_root(temp.path(), None).await.unwrap();
    std::fs::write(root.join("tracked"), "same contents").unwrap();
    git(&root, &["add", "tracked"]);
    git(
        &root,
        &[
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@localhost",
            "commit",
            "-m",
            "seed",
        ],
    );
    // Invalidate the index tree cache and working-file stat cache without
    // changing HEAD or index content. write-tree/status must not refresh them.
    git(&root, &["update-index", "--force-remove", "tracked"]);
    git(&root, &["add", "tracked"]);
    std::fs::write(root.join("tracked"), "same contents").unwrap();
    let index = root.join(".git/index");
    let before = (
        std::fs::read(&index).unwrap(),
        index.metadata().unwrap().modified().unwrap(),
    );
    let baseline = adapter.clean_baseline(&root).await.unwrap();
    assert_eq!(adapter.clean_baseline(&root).await.unwrap(), baseline);
    assert_eq!(
        (
            std::fs::read(&index).unwrap(),
            index.metadata().unwrap().modified().unwrap()
        ),
        before
    );
}

#[cfg(unix)]
#[tokio::test]
async fn a_rebound_checked_target_is_rejected_before_initialization() {
    let temp = tempfile::tempdir().unwrap();
    let parent = temp.path().canonicalize().unwrap();
    let expected = parent.join("checked");
    let retained = parent.join("retained");
    let unchecked = parent.join("unchecked");
    std::fs::create_dir(&expected).unwrap();
    std::fs::create_dir(&unchecked).unwrap();
    let adapter = LocalProjectWorkspace::default();
    assert_eq!(
        adapter
            .path_facts(&expected, &expected)
            .await
            .unwrap()
            .canonical_root,
        expected
    );
    std::fs::rename(&expected, &retained).unwrap();
    std::os::unix::fs::symlink(&unchecked, &expected).unwrap();
    let failure = adapter
        .prepare_git_root(&expected, Some(&expected))
        .await
        .unwrap_err();
    assert_eq!(failure.code, ErrorCode::RunQueueConflict);
    assert!(failure.retryable);
    assert!(adapter.verify_git_root(&expected).await.is_err());
    assert!(!unchecked.join(".git").exists());
    assert!(!retained.join(".git").exists());
}

#[tokio::test]
async fn dirty_index_untracked_and_existing_worktree_are_preserved() {
    let root = tempfile::tempdir().unwrap();
    let adapter = LocalProjectWorkspace::default();
    let primary = adapter.prepare_git_root(root.path(), None).await.unwrap();
    std::fs::write(primary.join("staged"), "keep staged").unwrap();
    git(&primary, &["add", "staged"]);
    assert_eq!(
        adapter.ensure_git_head(&primary).await.unwrap_err().code,
        ErrorCode::ProjectGitHeadUnavailable
    );
    assert!(adapter.git_head(&primary).await.unwrap().is_none());
    git(
        &primary,
        &[
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@localhost",
            "commit",
            "-m",
            "seed",
        ],
    );
    let head = adapter.clean_baseline(&primary).await.unwrap().commit;
    let worktree = primary.join(".ait/session");
    adapter
        .ensure_session_worktree(&primary, &worktree, &head, None)
        .await
        .unwrap();
    std::fs::write(worktree.join("staged"), "user modification").unwrap();
    assert_eq!(
        adapter.clean_baseline(&worktree).await.unwrap_err().code,
        ErrorCode::ProjectGitDirty
    );
    assert!(
        !adapter
            .ensure_session_worktree(&primary, &worktree, &head, None)
            .await
            .unwrap()
    );
    assert_eq!(
        std::fs::read_to_string(worktree.join("staged")).unwrap(),
        "user modification"
    );
    git(&worktree, &["add", "staged"]);
    assert_eq!(
        adapter.clean_baseline(&worktree).await.unwrap_err().code,
        ErrorCode::ProjectGitDirty
    );
    std::fs::write(primary.join("untracked"), "untouched").unwrap();
    assert_eq!(
        adapter.clean_baseline(&primary).await.unwrap_err().code,
        ErrorCode::ProjectGitDirty
    );
    assert_eq!(adapter.git_head(&primary).await.unwrap().unwrap(), head);
}

#[tokio::test]
async fn lease_child_process() {
    let Some(root) = std::env::var_os("AIT_PROJECT_LEASE_CONTRACT_ROOT") else {
        return;
    };
    let result = LocalProjectWorkspace::default()
        .acquire_lease(Path::new(&root))
        .await;
    assert_eq!(result.err().unwrap().code, ErrorCode::ProjectWorkspaceBusy);
}

#[tokio::test]
async fn leases_exclude_an_independent_process_and_cancelled_waiters_release() {
    let root = tempfile::tempdir().unwrap();
    let adapter = Arc::new(LocalProjectWorkspace::default());
    adapter.prepare_git_root(root.path(), None).await.unwrap();
    adapter.ensure_git_head(root.path()).await.unwrap();
    let lease = adapter.acquire_lease(root.path()).await.unwrap();
    let child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "lease_child_process", "--nocapture"])
        .env("AIT_PROJECT_LEASE_CONTRACT_ROOT", root.path())
        .output()
        .unwrap();
    assert!(
        child.status.success(),
        "{}",
        String::from_utf8_lossy(&child.stderr)
    );
    assert!(
        tokio::time::timeout(
            std::time::Duration::from_millis(40),
            adapter.acquire_lease(root.path())
        )
        .await
        .is_err()
    );
    drop(lease);
    let _next = adapter.acquire_lease(root.path()).await.unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn aliases_share_locks_and_path_facts_fail_closed_for_dangling_and_non_utf8() {
    use std::{
        ffi::OsString,
        os::unix::{ffi::OsStringExt, fs::symlink},
    };
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let adapter = LocalProjectWorkspace::default();
    let primary = adapter.prepare_git_root(root.path(), None).await.unwrap();
    adapter.ensure_git_head(&primary).await.unwrap();
    let alias = outside.path().join("alias");
    symlink(&primary, &alias).unwrap();
    let lease = adapter.acquire_lease(&primary).await.unwrap();
    assert_eq!(
        LocalProjectWorkspace::default()
            .acquire_lease(&alias)
            .await
            .err()
            .unwrap()
            .code,
        ErrorCode::ProjectWorkspaceBusy
    );
    let facts = adapter
        .path_facts(&alias, &alias.join("future/file"))
        .await
        .unwrap();
    assert_eq!(facts.canonical_root, primary);
    assert_eq!(facts.canonical_existing, primary);
    symlink(outside.path(), primary.join("escape")).unwrap();
    let facts = adapter
        .path_facts(&primary, &primary.join("escape/new"))
        .await
        .unwrap();
    assert!(!facts.canonical_existing.starts_with(&facts.canonical_root));
    symlink(outside.path().join("missing"), primary.join("dangling")).unwrap();
    assert!(
        adapter
            .path_facts(&primary, &primary.join("dangling/new"))
            .await
            .is_err()
    );
    let invalid = primary.join(OsString::from_vec(vec![0xff]));
    assert!(adapter.prepare_git_root(&invalid, None).await.is_err());
    assert!(
        adapter
            .path_facts(&primary, &invalid.join("new"))
            .await
            .is_err()
    );
    // APFS rejects non-UTF-8 directory entries at creation; Linux permits them.
    if std::fs::create_dir(&invalid).is_ok() {
        symlink(&invalid, primary.join("invalid-alias")).unwrap();
        assert!(
            adapter
                .path_facts(&primary, &primary.join("invalid-alias/new"))
                .await
                .is_err()
        );
    }
    drop(lease);
}
