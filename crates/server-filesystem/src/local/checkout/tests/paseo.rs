//! Behavioral ports of Paseo's checkout-git and commit-file-diff suites.

use std::fs;

use super::*;
use crate::ports::checkout::{CheckoutDiff, DiffLineKind, ParsedDiffStatus};

fn runtime(fixture: &Fixture) -> LocalCheckout {
    LocalCheckout::new(fixture.temp.path().join("managed"))
}

fn commit_file(fixture: &Fixture, name: &str, content: &str, subject: &str) {
    let path = fixture.repo.join(name);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
    git(&fixture.repo, &["add", "--", name]);
    git(
        &fixture.repo,
        &["-c", "commit.gpgsign=false", "commit", "-m", subject],
    );
}

fn diff(fixture: &Fixture, ignore_whitespace: bool) -> CheckoutDiff {
    runtime(fixture)
        .diff(
            fixture.repo.to_str().unwrap(),
            &CheckoutDiffCompare {
                mode: CheckoutDiffMode::Uncommitted,
                base_ref: None,
                ignore_whitespace,
            },
        )
        .unwrap()
}

fn origin(fixture: &Fixture) -> std::path::PathBuf {
    let remote = fixture.temp.path().join("origin.git");
    git(
        fixture.temp.path(),
        &["init", "--bare", "-b", "main", remote.to_str().unwrap()],
    );
    git(
        &fixture.repo,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    git(&fixture.repo, &["push", "-u", "origin", "main"]);
    remote
}

#[test]
fn unborn_checkout_returns_untracked_and_staged_files_in_diff() {
    let fixture = Fixture::new();
    git(&fixture.repo, &["checkout", "--orphan", "unborn"]);
    git(&fixture.repo, &["rm", "-rf", "."]);
    fs::write(fixture.repo.join("greeting.txt"), "hello\n").unwrap();
    fs::write(fixture.repo.join("staged.txt"), "staged\n").unwrap();
    git(&fixture.repo, &["add", "staged.txt"]);

    let result = diff(&fixture, false);
    assert_eq!(result.files.len(), 2);
    assert_eq!(result.files[0].path, "greeting.txt");
    assert_eq!(result.files[1].path, "staged.txt");
    assert!(
        result
            .files
            .iter()
            .all(|file| file.is_new && file.additions == 1)
    );
    assert_eq!(result.files[0].hunks[0].old_count, 0);
}

#[test]
fn whitespace_only_changes_can_be_hidden_without_hiding_real_edits() {
    let fixture = Fixture::new();
    fs::write(fixture.repo.join("tracked.txt"), "one  \n").unwrap();
    assert_eq!(diff(&fixture, false).files.len(), 1);
    assert!(diff(&fixture, true).files.is_empty());
    fs::write(fixture.repo.join("tracked.txt"), "two  \n").unwrap();
    assert_eq!(diff(&fixture, true).files[0].additions, 1);
}

#[test]
fn staged_rename_diff_keeps_both_paths_without_fabricating_hunks() {
    let fixture = Fixture::new();
    git(&fixture.repo, &["mv", "tracked.txt", "renamed.txt"]);
    let result = diff(&fixture, false);
    assert_eq!(result.files.len(), 1);
    assert_eq!(result.files[0].path, "renamed.txt");
    assert_eq!(result.files[0].old_path.as_deref(), Some("tracked.txt"));
    assert!(result.files[0].hunks.is_empty());
    assert!(!result.files[0].is_new);
    assert!(!result.files[0].is_deleted);
}

#[test]
fn no_prefix_git_configuration_preserves_a_and_b_directory_names() {
    let fixture = Fixture::new();
    for path in ["a/example.rs", "b/other.rs", "file with space.rs"] {
        commit_file(&fixture, path, "before\n", "add file");
        fs::write(fixture.repo.join(path), "after\n").unwrap();
    }
    git(&fixture.repo, &["config", "diff.noprefix", "true"]);
    let result = diff(&fixture, false);
    assert_eq!(
        result
            .files
            .iter()
            .map(|file| file.path.as_str())
            .collect::<Vec<_>>(),
        ["a/example.rs", "b/other.rs", "file with space.rs"]
    );
    assert!(result.files.iter().all(|file| file.hunks.len() == 1));
}

#[test]
fn tracked_binary_change_is_a_placeholder_without_text_hunks() {
    let fixture = Fixture::new();
    fs::write(fixture.repo.join("binary.dat"), b"first\0binary").unwrap();
    git(&fixture.repo, &["add", "binary.dat"]);
    git(&fixture.repo, &["commit", "-m", "binary"]);
    fs::write(fixture.repo.join("binary.dat"), b"second\0binary").unwrap();
    let result = diff(&fixture, false);
    assert_eq!(result.files.len(), 1);
    assert_eq!(result.files[0].path, "binary.dat");
    assert_eq!(result.files[0].status, Some(ParsedDiffStatus::Binary));
    assert!(result.files[0].hunks.is_empty());
}

#[test]
fn untracked_binary_change_is_a_new_placeholder() {
    let fixture = Fixture::new();
    fs::write(fixture.repo.join("binary.dat"), b"new\0binary").unwrap();
    let result = diff(&fixture, false);
    assert_eq!(result.files.len(), 1);
    assert_eq!(result.files[0].path, "binary.dat");
    assert_eq!(result.files[0].status, Some(ParsedDiffStatus::Binary));
    assert!(result.files[0].is_new);
    assert_eq!(result.files[0].additions, 0);
}

#[test]
fn empty_untracked_file_has_no_added_lines() {
    let fixture = Fixture::new();
    fs::write(fixture.repo.join("empty.txt"), "").unwrap();
    let result = diff(&fixture, false);
    assert_eq!(result.files.len(), 1);
    assert_eq!(result.files[0].path, "empty.txt");
    assert!(result.files[0].is_new);
    assert_eq!(result.files[0].additions, 0);
    assert!(result.files[0].hunks.is_empty());
}

#[test]
fn base_diff_uses_merge_base_and_does_not_delete_base_only_files() {
    let fixture = Fixture::new();
    git(&fixture.repo, &["checkout", "-b", "feature"]);
    commit_file(&fixture, "feature.txt", "feature\n", "feature");
    git(&fixture.repo, &["checkout", "main"]);
    commit_file(&fixture, "base-only.txt", "base\n", "base advance");
    git(&fixture.repo, &["checkout", "feature"]);
    let result = runtime(&fixture)
        .diff(
            fixture.repo.to_str().unwrap(),
            &CheckoutDiffCompare {
                mode: CheckoutDiffMode::Base,
                base_ref: Some("main".to_owned()),
                ignore_whitespace: false,
            },
        )
        .unwrap();
    assert_eq!(result.files.len(), 1);
    assert_eq!(result.files[0].path, "feature.txt");
    assert!(result.files[0].is_new);
}

#[test]
fn commit_message_quotes_and_shell_syntax_remain_literal() {
    let fixture = Fixture::new();
    fs::write(fixture.repo.join("tracked.txt"), "changed\n").unwrap();
    let message = "fix 'quoted' \"message\" $(touch escaped) `touch escaped2`";
    runtime(&fixture)
        .commit(fixture.repo.to_str().unwrap(), message, true)
        .unwrap();
    assert_eq!(
        git_output(&fixture.repo, &["log", "-1", "--format=%s"]),
        message
    );
    assert!(!fixture.repo.join("escaped").exists());
    assert!(!fixture.repo.join("escaped2").exists());
}

#[test]
fn commit_without_add_all_leaves_unstaged_files_out_of_the_commit() {
    let fixture = Fixture::new();
    fs::write(fixture.repo.join("tracked.txt"), "unstaged\n").unwrap();
    fs::write(fixture.repo.join("staged.txt"), "staged\n").unwrap();
    git(&fixture.repo, &["add", "staged.txt"]);
    runtime(&fixture)
        .commit(fixture.repo.to_str().unwrap(), "index only", false)
        .unwrap();
    assert_eq!(
        git_output(&fixture.repo, &["show", "HEAD:tracked.txt"]),
        "one"
    );
    assert_eq!(
        git_output(&fixture.repo, &["show", "HEAD:staged.txt"]),
        "staged"
    );
    assert_eq!(
        fs::read_to_string(fixture.repo.join("tracked.txt")).unwrap(),
        "unstaged\n"
    );
}

#[test]
fn commit_file_diff_is_none_when_the_commit_did_not_touch_the_path() {
    let fixture = Fixture::new();
    commit_file(&fixture, "other.txt", "new\n", "other change");
    let sha = git_output(&fixture.repo, &["rev-parse", "HEAD"]);
    assert_eq!(
        runtime(&fixture)
            .commit_file_diff(fixture.repo.to_str().unwrap(), &sha, "tracked.txt")
            .unwrap(),
        None
    );
}

#[test]
fn commit_file_diff_marks_added_and_deleted_files() {
    let fixture = Fixture::new();
    commit_file(&fixture, "new.txt", "new\n", "new file");
    let added_sha = git_output(&fixture.repo, &["rev-parse", "HEAD"]);
    git(&fixture.repo, &["rm", "new.txt"]);
    git(&fixture.repo, &["commit", "-m", "remove file"]);
    let removed_sha = git_output(&fixture.repo, &["rev-parse", "HEAD"]);
    let runtime = runtime(&fixture);
    let added = runtime
        .commit_file_diff(fixture.repo.to_str().unwrap(), &added_sha, "new.txt")
        .unwrap()
        .unwrap();
    let removed = runtime
        .commit_file_diff(fixture.repo.to_str().unwrap(), &removed_sha, "new.txt")
        .unwrap()
        .unwrap();
    assert!(added.is_new);
    assert_eq!(added.additions, 1);
    assert!(removed.is_deleted);
    assert_eq!(removed.deletions, 1);
    assert_eq!(removed.hunks[0].lines[1].kind, DiffLineKind::Remove);
}

#[test]
fn merge_commit_file_diff_compares_against_first_parent() {
    let fixture = Fixture::new();
    git(&fixture.repo, &["checkout", "-b", "feature"]);
    commit_file(&fixture, "feature.txt", "feature\n", "feature");
    git(&fixture.repo, &["checkout", "main"]);
    commit_file(&fixture, "base.txt", "base\n", "base");
    git(
        &fixture.repo,
        &["merge", "--no-ff", "feature", "-m", "merge feature"],
    );
    let sha = git_output(&fixture.repo, &["rev-parse", "HEAD"]);
    let runtime = runtime(&fixture);
    let result = runtime
        .commit_file_diff(fixture.repo.to_str().unwrap(), &sha, "feature.txt")
        .unwrap()
        .unwrap();
    assert!(result.is_new);
    assert_eq!(result.additions, 1);
    assert!(
        runtime
            .commit_file_diff(fixture.repo.to_str().unwrap(), &sha, "base.txt")
            .unwrap()
            .is_none()
    );
}

#[test]
fn local_history_has_no_remote_flags_without_remote_refs() {
    let fixture = Fixture::new();
    commit_file(&fixture, "second.txt", "second\n", "second");
    let result = runtime(&fixture)
        .commits(fixture.repo.to_str().unwrap())
        .unwrap();
    assert_eq!(result.commits.len(), 2);
    assert!(result.commits.iter().all(|commit| !commit.is_on_remote));
    assert_eq!(result.commits[0].subject, "second");
}

#[test]
fn base_history_is_bounded_to_the_ten_newest_commits() {
    let fixture = Fixture::new();
    for index in 0..12 {
        commit_file(
            &fixture,
            "counter",
            &index.to_string(),
            &format!("commit {index}"),
        );
    }
    let result = runtime(&fixture)
        .commits(fixture.repo.to_str().unwrap())
        .unwrap();
    assert_eq!(result.commits.len(), 10);
    assert!(result.commits.iter().all(|commit| commit.is_on_base));
    assert_eq!(result.commits[0].subject, "commit 11");
    assert_eq!(result.commits[9].subject, "commit 2");
}

#[test]
fn branch_resolution_does_not_treat_a_tag_as_a_branch() {
    let fixture = Fixture::new();
    git(&fixture.repo, &["tag", "release"]);
    assert_eq!(
        runtime(&fixture)
            .validate_branch(fixture.repo.to_str().unwrap(), "release")
            .unwrap(),
        CheckoutBranchResolution::NotFound
    );
    assert!(
        runtime(&fixture)
            .switch_branch(fixture.repo.to_str().unwrap(), "release")
            .is_err()
    );
    assert_eq!(
        git_output(&fixture.repo, &["symbolic-ref", "--short", "HEAD"]),
        "main"
    );
}

#[test]
fn branch_rename_collision_preserves_both_branch_tips() {
    let fixture = Fixture::new();
    git(&fixture.repo, &["branch", "occupied"]);
    let head = git_output(&fixture.repo, &["rev-parse", "HEAD"]);
    assert!(
        runtime(&fixture)
            .rename_branch(fixture.repo.to_str().unwrap(), "occupied")
            .is_err()
    );
    assert_eq!(
        git_output(&fixture.repo, &["symbolic-ref", "--short", "HEAD"]),
        "main"
    );
    assert_eq!(git_output(&fixture.repo, &["rev-parse", "occupied"]), head);
}

#[test]
fn local_branch_wins_over_a_divergent_origin_branch() {
    let fixture = Fixture::new();
    origin(&fixture);
    git(&fixture.repo, &["checkout", "-b", "feature"]);
    git(&fixture.repo, &["push", "origin", "feature"]);
    commit_file(&fixture, "local.txt", "local\n", "local feature");
    let head = git_output(&fixture.repo, &["rev-parse", "HEAD"]);
    git(&fixture.repo, &["checkout", "main"]);
    assert_eq!(
        runtime(&fixture)
            .switch_branch(fixture.repo.to_str().unwrap(), "origin/feature")
            .unwrap(),
        CheckoutBranchSource::Local
    );
    assert_eq!(git_output(&fixture.repo, &["rev-parse", "HEAD"]), head);
}

#[test]
fn branch_suggestions_expose_divergence_and_respect_query_limit() {
    let fixture = Fixture::new();
    origin(&fixture);
    git(&fixture.repo, &["branch", "feature-one"]);
    git(&fixture.repo, &["branch", "feature-two"]);
    commit_file(&fixture, "local.txt", "local\n", "local");
    let runtime = runtime(&fixture);
    let branches = runtime
        .branch_suggestions(fixture.repo.to_str().unwrap(), Some("MAIN"), 10)
        .unwrap();
    assert_eq!(branches.len(), 1);
    assert!(branches[0].has_local && branches[0].has_remote);
    assert_eq!(branches[0].local_ahead, Some(1));
    assert_eq!(branches[0].local_behind, Some(0));
    let limited = runtime
        .branch_suggestions(fixture.repo.to_str().unwrap(), Some("feature"), 1)
        .unwrap();
    assert_eq!(limited.len(), 1);
    assert!(limited[0].name.starts_with("feature-"));
}

#[test]
fn discard_staged_rename_restores_both_sides() {
    let fixture = Fixture::new();
    git(&fixture.repo, &["mv", "tracked.txt", "renamed.txt"]);
    runtime(&fixture)
        .discard_changes(
            fixture.repo.to_str().unwrap(),
            &["tracked.txt".to_owned(), "renamed.txt".to_owned()],
        )
        .unwrap();
    assert_eq!(
        fs::read_to_string(fixture.repo.join("tracked.txt")).unwrap(),
        "one\n"
    );
    assert!(!fixture.repo.join("renamed.txt").exists());
    assert!(git_output(&fixture.repo, &["status", "--porcelain"]).is_empty());
}

#[test]
fn discard_unborn_index_removes_only_requested_files() {
    let fixture = Fixture::new();
    git(&fixture.repo, &["checkout", "--orphan", "unborn"]);
    git(&fixture.repo, &["rm", "-rf", "."]);
    fs::write(fixture.repo.join("new.txt"), "remove\n").unwrap();
    fs::write(fixture.repo.join("keep.txt"), "keep\n").unwrap();
    git(&fixture.repo, &["add", "."]);
    runtime(&fixture)
        .discard_changes(fixture.repo.to_str().unwrap(), &["new.txt".to_owned()])
        .unwrap();
    assert!(!fixture.repo.join("new.txt").exists());
    assert_eq!(git_output(&fixture.repo, &["ls-files"]), "keep.txt");
    assert_eq!(
        fs::read_to_string(fixture.repo.join("keep.txt")).unwrap(),
        "keep\n"
    );
}

#[test]
fn discard_nested_directory_preserves_sibling_changes() {
    let fixture = Fixture::new();
    commit_file(&fixture, "nested/tracked", "original\n", "nested");
    fs::write(fixture.repo.join("nested/tracked"), "changed\n").unwrap();
    fs::write(fixture.repo.join("nested/untracked"), "remove\n").unwrap();
    fs::write(fixture.repo.join("outside"), "keep\n").unwrap();
    runtime(&fixture)
        .discard_changes(fixture.repo.to_str().unwrap(), &["nested".to_owned()])
        .unwrap();
    assert_eq!(
        fs::read_to_string(fixture.repo.join("nested/tracked")).unwrap(),
        "original\n"
    );
    assert!(!fixture.repo.join("nested/untracked").exists());
    assert_eq!(
        fs::read_to_string(fixture.repo.join("outside")).unwrap(),
        "keep\n"
    );
}

#[test]
fn discard_staged_deletion_restores_the_committed_content() {
    let fixture = Fixture::new();
    git(&fixture.repo, &["rm", "tracked.txt"]);
    runtime(&fixture)
        .discard_changes(fixture.repo.to_str().unwrap(), &["tracked.txt".to_owned()])
        .unwrap();
    assert_eq!(
        fs::read_to_string(fixture.repo.join("tracked.txt")).unwrap(),
        "one\n"
    );
    assert!(git_output(&fixture.repo, &["status", "--porcelain"]).is_empty());
}

#[test]
fn discard_filename_globs_are_literal_and_do_not_touch_matching_siblings() {
    let fixture = Fixture::new();
    commit_file(&fixture, "foo[ab].txt", "literal\n", "literal file");
    commit_file(&fixture, "fooa.txt", "sibling\n", "sibling file");
    fs::write(fixture.repo.join("foo[ab].txt"), "changed literal\n").unwrap();
    fs::write(fixture.repo.join("fooa.txt"), "changed sibling\n").unwrap();
    runtime(&fixture)
        .discard_changes(fixture.repo.to_str().unwrap(), &["foo[ab].txt".to_owned()])
        .unwrap();
    assert_eq!(
        fs::read_to_string(fixture.repo.join("foo[ab].txt")).unwrap(),
        "literal\n"
    );
    assert_eq!(
        fs::read_to_string(fixture.repo.join("fooa.txt")).unwrap(),
        "changed sibling\n"
    );
}

#[test]
fn push_honors_a_differently_named_configured_upstream_branch() {
    let fixture = Fixture::new();
    let remote = origin(&fixture);
    git(&fixture.repo, &["push", "-u", "origin", "main:review"]);
    commit_file(&fixture, "local.txt", "new\n", "local");
    runtime(&fixture)
        .push(fixture.repo.to_str().unwrap())
        .unwrap();
    assert_eq!(
        git_output(&remote, &["rev-parse", "review"]),
        git_output(&fixture.repo, &["rev-parse", "HEAD"])
    );
    assert_ne!(
        git_output(&remote, &["rev-parse", "main"]),
        git_output(&fixture.repo, &["rev-parse", "HEAD"])
    );
}

#[test]
fn unborn_sha256_repository_uses_its_own_empty_tree_identity() {
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path().join("sha256");
    fs::create_dir(&repo).unwrap();
    git(&repo, &["init", "--object-format=sha256", "-b", "main"]);
    let fixture = Fixture { temp, repo };
    fs::write(fixture.repo.join("new.txt"), "sha256\n").unwrap();
    git(&fixture.repo, &["add", "new.txt"]);
    let result = diff(&fixture, false);
    assert_eq!(result.files.len(), 1);
    assert_eq!(result.files[0].path, "new.txt");
    assert!(result.files[0].is_new);
    assert_eq!(result.files[0].additions, 1);
}

#[test]
fn commit_file_diff_preserves_spaces_in_the_requested_path() {
    let fixture = Fixture::new();
    commit_file(&fixture, "file with space.txt", "before\n", "add");
    commit_file(&fixture, "file with space.txt", "after\n", "edit");
    let sha = git_output(&fixture.repo, &["rev-parse", "HEAD"]);
    let changed = runtime(&fixture)
        .commit_file_diff(fixture.repo.to_str().unwrap(), &sha, "file with space.txt")
        .unwrap()
        .unwrap();
    assert_eq!(changed.path, "file with space.txt");
    assert_eq!(changed.additions, 1);
    assert_eq!(changed.deletions, 1);
}
