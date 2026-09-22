use serde_json::json;
use server_application::checkout::{
    AheadBehind, Checkout, CheckoutBranchResolution, CheckoutBranchSource,
    CheckoutBranchSuggestion, CheckoutCommit, CheckoutCommitFile, CheckoutCommitFileStatus,
    CheckoutCommits, CheckoutDiff, CheckoutDiffCompare, CheckoutFailureKind, CheckoutMergeStrategy,
    CheckoutRuntime, CheckoutRuntimeError, CheckoutStashEntry, CheckoutStatus, ParsedDiffFile,
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

#[test]
fn branch_queries_and_mutations_project_paseo_shapes() {
    let checkout = checkout();
    let validation = execute(
        &checkout,
        "checkout.branch.validate.request",
        json!({"cwd":"/repo","branchName":"feature"}),
    )
    .unwrap()
    .value;
    assert_eq!(
        validation,
        json!({
            "exists":true,"resolvedRef":"feature","isRemote":false,"error":null
        })
    );

    let suggestions = execute(
        &checkout,
        "checkout.branch.suggestions.request",
        json!({"cwd":"/repo"}),
    )
    .unwrap()
    .value;
    assert_eq!(suggestions["branches"], json!(["feature"]));
    assert_eq!(suggestions["branchDetails"][0]["hasLocal"], true);

    let switched = execute(
        &checkout,
        "checkout.branch.switch.request",
        json!({"cwd":"/repo","branch":"feature"}),
    )
    .unwrap()
    .value;
    assert_eq!(switched["source"], "local");
    assert_eq!(switched["success"], true);

    let renamed = execute(
        &checkout,
        "checkout.rename_branch.request",
        json!({"cwd":"/repo","branch":"renamed"}),
    )
    .unwrap()
    .value;
    assert_eq!(renamed["currentBranch"], "renamed");
}

#[test]
fn mutation_defaults_and_inline_errors_match_paseo() {
    let checkout = checkout();
    for (method, params) in [
        (
            "checkout.commit.request",
            json!({"cwd":"/repo","message":"ship"}),
        ),
        ("checkout.merge.request", json!({"cwd":"/repo"})),
        ("checkout.merge_from_base.request", json!({"cwd":"/repo"})),
        ("checkout.pull.request", json!({"cwd":"/repo"})),
        ("checkout.push.request", json!({"cwd":"/repo"})),
        (
            "checkout.discard_changes.request",
            json!({"cwd":"/repo","paths":["a.txt"]}),
        ),
        ("checkout.stash.save.request", json!({"cwd":"/repo"})),
        (
            "checkout.stash.pop.request",
            json!({"cwd":"/repo","stashIndex":0}),
        ),
    ] {
        let result = execute(&checkout, method, params).unwrap().value;
        assert_eq!(result, json!({"cwd":"/repo","success":true,"error":null}));
    }
    let failed = execute(&checkout, "checkout.commit.request", json!({"cwd":"fail"}))
        .unwrap()
        .value;
    assert_eq!(failed["success"], false);
    assert_eq!(failed["error"]["code"], "NOT_GIT_REPO");
    assert!(
        execute(
            &checkout,
            "checkout.discard_changes.request",
            json!({"cwd":"/repo","paths":[]}),
        )
        .is_err()
    );
}

#[test]
fn stash_list_defaults_to_paseo_only_and_preserves_entry_shape() {
    let result = execute(
        &checkout(),
        "checkout.stash.list.request",
        json!({"cwd":"/repo"}),
    )
    .unwrap()
    .value;
    assert_eq!(result["entries"][0]["branch"], "feature");
    assert_eq!(result["entries"][0]["isPaseo"], true);
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

    fn validate_branch(
        &self,
        cwd: &str,
        branch: &str,
    ) -> Result<CheckoutBranchResolution, CheckoutRuntimeError> {
        fail(cwd)?;
        Ok(CheckoutBranchResolution::Local(branch.to_owned()))
    }

    fn branch_suggestions(
        &self,
        cwd: &str,
        _query: Option<&str>,
        _limit: usize,
    ) -> Result<Vec<CheckoutBranchSuggestion>, CheckoutRuntimeError> {
        fail(cwd)?;
        Ok(vec![CheckoutBranchSuggestion {
            name: "feature".to_owned(),
            committer_date: 1,
            has_local: true,
            has_remote: false,
            local_ahead: None,
            local_behind: None,
        }])
    }

    fn switch_branch(
        &self,
        cwd: &str,
        _branch: &str,
    ) -> Result<CheckoutBranchSource, CheckoutRuntimeError> {
        fail(cwd)?;
        Ok(CheckoutBranchSource::Local)
    }

    fn rename_branch(&self, cwd: &str, branch: &str) -> Result<String, CheckoutRuntimeError> {
        fail(cwd)?;
        Ok(branch.to_owned())
    }

    fn commit(
        &self,
        cwd: &str,
        _message: &str,
        _add_all: bool,
    ) -> Result<(), CheckoutRuntimeError> {
        fail(cwd)
    }

    fn merge_to_base(
        &self,
        cwd: &str,
        _base_ref: Option<&str>,
        _strategy: CheckoutMergeStrategy,
        _require_clean_target: bool,
    ) -> Result<(), CheckoutRuntimeError> {
        fail(cwd)
    }

    fn merge_from_base(
        &self,
        cwd: &str,
        _base_ref: Option<&str>,
        _require_clean_target: bool,
    ) -> Result<(), CheckoutRuntimeError> {
        fail(cwd)
    }

    fn pull(&self, cwd: &str) -> Result<(), CheckoutRuntimeError> {
        fail(cwd)
    }

    fn push(&self, cwd: &str) -> Result<(), CheckoutRuntimeError> {
        fail(cwd)
    }

    fn discard_changes(&self, cwd: &str, _paths: &[String]) -> Result<(), CheckoutRuntimeError> {
        fail(cwd)
    }

    fn stash_save(&self, cwd: &str, _branch: Option<&str>) -> Result<(), CheckoutRuntimeError> {
        fail(cwd)
    }

    fn stash_pop(&self, cwd: &str, _index: usize) -> Result<(), CheckoutRuntimeError> {
        fail(cwd)
    }

    fn stashes(
        &self,
        cwd: &str,
        _paseo_only: bool,
    ) -> Result<Vec<CheckoutStashEntry>, CheckoutRuntimeError> {
        fail(cwd)?;
        Ok(vec![CheckoutStashEntry {
            index: 0,
            message: "paseo-auto-stash: feature".to_owned(),
            branch: Some("feature".to_owned()),
            is_paseo: true,
        }])
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
