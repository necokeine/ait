use serde_json::json;
use server_application::checkout::{
    AheadBehind, Checkout, CheckoutCommit, CheckoutCommitFile, CheckoutCommitFileStatus,
    CheckoutCommits, CheckoutDiff, CheckoutDiffCompare, CheckoutFailureKind, CheckoutRuntime,
    CheckoutRuntimeError, CheckoutStatus, ParsedDiffFile,
};

use super::execute;

#[test]
fn status_projection_preserves_managed_checkout_facts() {
    let checkout = checkout();
    let result = execute(
        &checkout,
        "checkout.status.get.request",
        json!({"cwd":"/repo"}),
    )
    .unwrap()
    .value;
    assert_eq!(result["isGit"], true);
    assert_eq!(result["isPaseoOwnedWorktree"], true);
    assert_eq!(result["mainRepoRoot"], "/main");
    assert_eq!(result["aheadBehind"], json!({"ahead":2,"behind":1}));
}

#[test]
fn checkout_failures_stay_inline_with_paseo_error_codes() {
    let checkout = checkout();
    let diff = execute(
        &checkout,
        "checkout.diff.get.request",
        json!({"cwd":"fail","compare":{"mode":"uncommitted"}}),
    )
    .unwrap()
    .value;
    assert_eq!(diff["files"], json!([]));
    assert_eq!(diff["error"]["code"], "NOT_GIT_REPO");

    let refresh = execute(&checkout, "checkout.refresh.request", json!({"cwd":"fail"}))
        .unwrap()
        .value;
    assert_eq!(refresh["success"], false);
    assert_eq!(refresh["error"]["code"], "NOT_GIT_REPO");
}

#[test]
fn commits_project_files_and_malformed_requests_are_rejected() {
    let checkout = checkout();
    let result = execute(
        &checkout,
        "checkout.commits.list.request",
        json!({"cwd":"/repo"}),
    )
    .unwrap()
    .value;
    assert_eq!(result["baseRef"], "main");
    assert_eq!(result["commits"][0]["files"][0]["status"], "modified");
    assert!(
        execute(
            &checkout,
            "checkout.commits.file_diff.request",
            json!({"cwd":"/repo","sha":"abc"}),
        )
        .is_err()
    );
}

fn checkout() -> Checkout {
    Checkout::new(Box::new(FakeCheckout))
}

#[derive(Debug)]
struct FakeCheckout;

impl CheckoutRuntime for FakeCheckout {
    fn status(&self, cwd: &str) -> Result<CheckoutStatus, CheckoutRuntimeError> {
        fail(cwd)?;
        Ok(CheckoutStatus {
            is_git: true,
            repo_root: Some("/repo".to_owned()),
            main_repo_root: Some("/main".to_owned()),
            current_branch: Some("feature".to_owned()),
            is_dirty: Some(true),
            base_ref: Some("main".to_owned()),
            ahead_behind: Some(AheadBehind {
                ahead: 2,
                behind: 1,
            }),
            upstream_ref: Some("refs/remotes/origin/feature".to_owned()),
            ahead_of_origin: Some(1),
            behind_of_origin: Some(0),
            has_remote: true,
            remote_url: Some("git@example.test:org/repo.git".to_owned()),
            is_managed_worktree: true,
        })
    }

    fn refresh(&self, cwd: &str) -> Result<(), CheckoutRuntimeError> {
        fail(cwd)
    }

    fn diff(
        &self,
        cwd: &str,
        _compare: &CheckoutDiffCompare,
    ) -> Result<CheckoutDiff, CheckoutRuntimeError> {
        fail(cwd)?;
        Ok(CheckoutDiff {
            files: Vec::new(),
            diff_too_large: false,
        })
    }

    fn commits(&self, cwd: &str) -> Result<CheckoutCommits, CheckoutRuntimeError> {
        fail(cwd)?;
        Ok(CheckoutCommits {
            base_ref: Some("main".to_owned()),
            commits: vec![CheckoutCommit {
                sha: "1".repeat(40),
                short_sha: "1111111".to_owned(),
                subject: "subject".to_owned(),
                author_name: "Author".to_owned(),
                author_date: "2026-01-01T00:00:00Z".to_owned(),
                is_on_remote: false,
                is_on_base: false,
                files: vec![CheckoutCommitFile {
                    path: "src/main.rs".to_owned(),
                    additions: 1,
                    deletions: 1,
                    status: Some(CheckoutCommitFileStatus::Modified),
                }],
            }],
        })
    }

    fn commit_file_diff(
        &self,
        cwd: &str,
        _sha: &str,
        _path: &str,
    ) -> Result<Option<ParsedDiffFile>, CheckoutRuntimeError> {
        fail(cwd)?;
        Ok(None)
    }
}

fn fail(cwd: &str) -> Result<(), CheckoutRuntimeError> {
    if cwd == "fail" {
        Err(CheckoutRuntimeError {
            kind: CheckoutFailureKind::NotGitRepository,
            message: "Not a git repository".to_owned(),
        })
    } else {
        Ok(())
    }
}
