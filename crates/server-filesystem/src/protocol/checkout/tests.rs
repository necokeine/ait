use serde_json::json;

use super::*;

#[test]
fn capabilities_use_only_canonical_checkout_names() {
    assert_eq!(
        CAPABILITIES,
        [
            "checkout.status.get.request",
            "checkout.refresh.request",
            "checkout.diff.get.request",
            "checkout.diff.subscribe.request",
            "checkout.diff.unsubscribe.request",
            "checkout.commits.list.request",
            "checkout.commits.file_diff.request",
            "checkout.branch.validate.request",
            "checkout.branch.suggestions.request",
            "checkout.branch.switch.request",
            "checkout.rename_branch.request",
            "checkout.commit.request",
            "checkout.merge.request",
            "checkout.merge_from_base.request",
            "checkout.pull.request",
            "checkout.push.request",
            "checkout.discard_changes.request",
            "checkout.stash.save.request",
            "checkout.stash.pop.request",
            "checkout.stash.list.request",
        ]
    );
    assert!(!CAPABILITIES.contains(&"checkout_status_request"));
    assert!(!CAPABILITIES.contains(&"subscribe_checkout_diff_request"));
}

#[test]
fn mutation_requests_match_paseo_optional_fields_and_camel_case() {
    let commit: CheckoutCommitRequest = serde_json::from_value(json!({
        "cwd":"/repo","message":"ship","addAll":false
    }))
    .unwrap();
    assert_eq!(commit.message.as_deref(), Some("ship"));
    assert_eq!(commit.add_all, Some(false));

    let merge: CheckoutMergeRequest = serde_json::from_value(json!({
        "cwd":"/repo","baseRef":"origin/main","strategy":"squash",
        "requireCleanTarget":true
    }))
    .unwrap();
    assert_eq!(merge.strategy, Some(CheckoutMergeStrategy::Squash));
    assert_eq!(merge.require_clean_target, Some(true));

    let stash: CheckoutStashListRequest = serde_json::from_value(json!({"cwd":"/repo"})).unwrap();
    assert_eq!(stash.paseo_only, None);
}

#[test]
fn branch_and_stash_results_match_paseo_shapes() {
    let suggestions = serde_json::to_value(CheckoutBranchSuggestionsResult {
        branches: vec!["main".to_owned()],
        branch_details: Some(vec![CheckoutBranchSuggestion {
            name: "main".to_owned(),
            committer_date: 7,
            has_local: true,
            has_remote: true,
            local_ahead: Some(1),
            local_behind: Some(2),
        }]),
        error: None,
    })
    .unwrap();
    assert_eq!(suggestions["branchDetails"][0]["committerDate"], 7);
    assert_eq!(suggestions["branchDetails"][0]["localBehind"], 2);

    let stashes = serde_json::to_value(CheckoutStashListResult {
        cwd: "/repo".to_owned(),
        entries: vec![CheckoutStashEntry {
            index: 0,
            message: "paseo-auto-stash: feature".to_owned(),
            branch: Some("feature".to_owned()),
            is_paseo: true,
        }],
        error: None,
    })
    .unwrap();
    assert_eq!(stashes["entries"][0]["isPaseo"], true);
}

#[test]
fn diff_requests_match_paseo_defaults_and_camel_case() {
    let request: CheckoutDiffSubscribeRequest = serde_json::from_value(json!({
        "subscriptionId":"diff-1",
        "cwd":"/repo",
        "compare":{"mode":"base","baseRef":"origin/main"},
        "future":"ignored"
    }))
    .unwrap();
    assert_eq!(request.subscription_id.as_deref(), Some("diff-1"));
    assert_eq!(request.compare.mode, CheckoutDiffMode::Base);
    assert_eq!(request.compare.base_ref.as_deref(), Some("origin/main"));
    assert!(!request.compare.ignore_whitespace);

    let one_shot: CheckoutDiffGetRequest = serde_json::from_value(json!({
        "cwd":"/repo",
        "compare":{"mode":"uncommitted","ignoreWhitespace":true}
    }))
    .unwrap();
    assert!(one_shot.compare.ignore_whitespace);
}

#[test]
fn checkout_status_preserves_null_non_git_shape_and_uppercase_errors() {
    let value = serde_json::to_value(CheckoutStatusResult {
        cwd: "/tmp/plain".to_owned(),
        is_git: false,
        repo_root: None,
        main_repo_root: None,
        current_branch: None,
        is_dirty: None,
        base_ref: None,
        ahead_behind: None,
        upstream_ref: None,
        ahead_of_origin: None,
        behind_of_origin: None,
        has_remote: false,
        remote_url: None,
        is_paseo_owned_worktree: false,
        error: Some(CheckoutError {
            code: CheckoutErrorCode::NotGitRepo,
            message: "not a repository".to_owned(),
        }),
    })
    .unwrap();
    assert_eq!(value["isGit"], false);
    assert!(value["repoRoot"].is_null());
    assert!(value.get("mainRepoRoot").is_none());
    assert_eq!(value["error"]["code"], "NOT_GIT_REPO");

    let value = serde_json::to_value(CheckoutStatusResult {
        cwd: "/repo".to_owned(),
        is_git: true,
        repo_root: Some("/repo".to_owned()),
        main_repo_root: None,
        current_branch: Some("main".to_owned()),
        is_dirty: Some(false),
        base_ref: None,
        ahead_behind: None,
        upstream_ref: None,
        ahead_of_origin: None,
        behind_of_origin: None,
        has_remote: false,
        remote_url: None,
        is_paseo_owned_worktree: false,
        error: None,
    })
    .unwrap();
    assert!(value.get("mainRepoRoot").is_some());
    assert!(value["mainRepoRoot"].is_null());
}

#[test]
fn structured_diff_and_commit_shapes_match_paseo_fields() {
    let file = ParsedDiffFile {
        path: "src/new.rs".to_owned(),
        old_path: Some("src/old.rs".to_owned()),
        is_new: false,
        is_deleted: false,
        additions: 1,
        deletions: 1,
        hunks: vec![DiffHunk {
            old_start: 1,
            old_count: 1,
            new_start: 1,
            new_count: 1,
            lines: vec![DiffLine {
                kind: DiffLineKind::Header,
                content: "@@ -1 +1 @@".to_owned(),
                tokens: None,
            }],
        }],
        status: None,
    };
    let value = serde_json::to_value(CheckoutCommitFileDiffResult {
        cwd: "/repo".to_owned(),
        sha: "1".repeat(40),
        path: "src/new.rs".to_owned(),
        file: Some(file),
        error: None,
    })
    .unwrap();
    assert_eq!(value["file"]["oldPath"], "src/old.rs");
    assert_eq!(value["file"]["hunks"][0]["oldStart"], 1);
    assert_eq!(value["file"]["hunks"][0]["lines"][0]["type"], "header");
    assert!(
        value["file"]["hunks"][0]["lines"][0]
            .get("tokens")
            .is_none()
    );
}

#[test]
fn commit_file_diff_request_rejects_missing_required_fields() {
    assert!(
        serde_json::from_value::<CheckoutCommitFileDiffRequest>(json!({
            "cwd":"/repo","sha":"abc"
        }))
        .is_err()
    );
    let request: CheckoutCommitFileDiffRequest = serde_json::from_value(json!({
        "cwd":"/repo","sha":"abc","path":"src/a.rs","unknown":true
    }))
    .unwrap();
    assert_eq!(request.path, "src/a.rs");
}
