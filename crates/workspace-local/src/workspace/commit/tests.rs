//! Git finalization changes refs/index only after a durable, repeatable plan exists.
use crate::LocalProjectWorkspace;
use ait_workspace::ProjectWorkspace;
use std::path::Path;

fn git(path: &Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(path)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().into()
}

async fn fixture() -> (
    tempfile::TempDir,
    LocalProjectWorkspace,
    ait_workspace::RunCommitBaseline,
) {
    let dir = tempfile::tempdir().unwrap();
    let workspace = LocalProjectWorkspace::default();
    workspace.prepare_git_root(dir.path(), None).await.unwrap();
    workspace.ensure_git_head(dir.path()).await.unwrap();
    let baseline = workspace.capture_run_commit(dir.path()).await.unwrap();
    (dir, workspace, baseline)
}

#[tokio::test]
async fn prepare_keeps_head_and_index_and_publication_is_idempotent() {
    let (dir, workspace, baseline) = fixture().await;
    std::fs::write(dir.path().join("result.txt"), "native result").unwrap();
    let plan = workspace
        .prepare_run_commit(dir.path(), &baseline, "run-1")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(git(dir.path(), &["rev-parse", "HEAD"]), baseline.head);
    assert_eq!(git(dir.path(), &["write-tree"]), baseline.index_tree);
    workspace
        .publish_run_commit(dir.path(), &plan)
        .await
        .unwrap();
    workspace
        .publish_run_commit(dir.path(), &plan)
        .await
        .unwrap();
    assert_eq!(git(dir.path(), &["rev-parse", "HEAD"]), plan.commit_id);
    assert_eq!(git(dir.path(), &["rev-list", "--count", "HEAD"]), "2");
    assert!(git(dir.path(), &["status", "--porcelain"]).is_empty());
}

#[tokio::test]
async fn lost_ack_after_ref_update_recovers_the_original_commit_and_index() {
    let (dir, workspace, baseline) = fixture().await;
    std::fs::write(dir.path().join("result.txt"), "native result").unwrap();
    let plan = workspace
        .prepare_run_commit(dir.path(), &baseline, "run-2")
        .await
        .unwrap()
        .unwrap();
    git(
        dir.path(),
        &["update-ref", "HEAD", &plan.commit_id, &baseline.head],
    );
    workspace
        .publish_run_commit(dir.path(), &plan)
        .await
        .unwrap();
    assert_eq!(git(dir.path(), &["rev-list", "--count", "HEAD"]), "2");
    assert!(git(dir.path(), &["status", "--porcelain"]).is_empty());
}

#[tokio::test]
async fn no_changes_skip_and_external_staging_or_head_changes_are_preserved() {
    let (dir, workspace, baseline) = fixture().await;
    assert!(
        workspace
            .prepare_run_commit(dir.path(), &baseline, "run")
            .await
            .unwrap()
            .is_none()
    );
    std::fs::write(dir.path().join("manual.txt"), "manual change").unwrap();
    assert!(workspace.capture_run_commit(dir.path()).await.is_err());
    git(dir.path(), &["add", "manual.txt"]);
    assert!(
        workspace
            .prepare_run_commit(dir.path(), &baseline, "run")
            .await
            .is_err()
    );
    assert_ne!(git(dir.path(), &["write-tree"]), baseline.index_tree);
    assert_eq!(git(dir.path(), &["rev-parse", "HEAD"]), baseline.head);
}

#[tokio::test]
async fn changed_files_after_prepare_are_not_silently_committed() {
    let (dir, workspace, baseline) = fixture().await;
    std::fs::write(dir.path().join("result.txt"), "generated").unwrap();
    let plan = workspace
        .prepare_run_commit(dir.path(), &baseline, "run")
        .await
        .unwrap()
        .unwrap();
    std::fs::write(dir.path().join("result.txt"), "concurrent manual edit").unwrap();
    assert!(
        workspace
            .publish_run_commit(dir.path(), &plan)
            .await
            .is_err()
    );
    assert_eq!(git(dir.path(), &["rev-parse", "HEAD"]), baseline.head);
    assert_eq!(
        std::fs::read_to_string(dir.path().join("result.txt")).unwrap(),
        "concurrent manual edit"
    );
}

#[tokio::test]
async fn crash_owned_index_lock_can_be_recovered_without_removing_an_external_lock() {
    let (dir, workspace, baseline) = fixture().await;
    std::fs::write(dir.path().join("result.txt"), "result").unwrap();
    let plan = workspace
        .prepare_run_commit(dir.path(), &baseline, "crash")
        .await
        .unwrap()
        .unwrap();
    let git_dir = std::path::PathBuf::from(git(dir.path(), &["rev-parse", "--absolute-git-dir"]));
    let receipt = git_dir.join(format!("ait-commit-{}.index", plan.commit_id));
    let result = std::process::Command::new("git")
        .arg("-C")
        .arg(dir.path())
        .env("GIT_INDEX_FILE", &receipt)
        .args(["read-tree", &plan.tree])
        .output()
        .unwrap();
    assert!(result.status.success());
    std::fs::hard_link(&receipt, git_dir.join("index.lock")).unwrap();
    git(
        dir.path(),
        &["update-ref", "HEAD", &plan.commit_id, &baseline.head],
    );
    workspace
        .publish_run_commit(dir.path(), &plan)
        .await
        .unwrap();
    assert!(!git_dir.join("index.lock").exists());
    assert!(git(dir.path(), &["status", "--porcelain"]).is_empty());
    std::fs::write(git_dir.join("index.lock"), "manual lock").unwrap();
    workspace
        .publish_run_commit(dir.path(), &plan)
        .await
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(git_dir.join("index.lock")).unwrap(),
        "manual lock"
    );
}

#[tokio::test]
async fn changed_branch_with_same_head_does_not_publish() {
    let (dir, workspace, baseline) = fixture().await;
    std::fs::write(dir.path().join("result.txt"), "result").unwrap();
    let plan = workspace
        .prepare_run_commit(dir.path(), &baseline, "branch")
        .await
        .unwrap()
        .unwrap();
    git(dir.path(), &["checkout", "-qb", "other"]);
    assert!(
        workspace
            .publish_run_commit(dir.path(), &plan)
            .await
            .is_err()
    );
    assert_eq!(git(dir.path(), &["rev-parse", "HEAD"]), baseline.head);
}
