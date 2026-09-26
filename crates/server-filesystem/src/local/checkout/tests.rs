use std::path::Path;
use std::process::Command;

use tempfile::TempDir;

mod paseo;

use super::{LocalCheckout, is_below};
use crate::ports::checkout::{
    CheckoutBranchResolution, CheckoutBranchSource, CheckoutCommitFileStatus, CheckoutDiffCompare,
    CheckoutDiffMode, CheckoutFailureKind, CheckoutMergeStrategy, CheckoutRuntime,
};

#[test]
fn managed_checkout_requires_repository_hash_and_slug_components() {
    let root = tempfile::tempdir().unwrap();
    let managed = root.path().join("worktrees");
    let project = managed.join("repository-hash");
    let checkout = project.join("feature");
    std::fs::create_dir_all(&checkout).unwrap();
    let nested = checkout.join("nested");
    std::fs::create_dir(&nested).unwrap();

    assert!(is_below(&managed, &checkout));
    assert!(!is_below(&managed, &project));
    assert!(!is_below(&managed, &nested));
    assert!(!is_below(&managed, root.path()));
}

#[test]
fn status_marks_only_linked_worktrees_in_the_managed_layout_as_owned() {
    let fixture = Fixture::new();
    let managed = fixture.temp.path().join("worktrees");
    let linked = managed.join("repository-hash").join("feature");
    std::fs::create_dir_all(linked.parent().unwrap()).unwrap();
    git(
        &fixture.repo,
        &["worktree", "add", "-b", "feature", linked.to_str().unwrap()],
    );
    let standalone = managed.join("other-hash").join("standalone");
    std::fs::create_dir_all(&standalone).unwrap();
    git(&standalone, &["init", "-b", "main"]);

    let runtime = LocalCheckout::new(managed);
    let linked_status = runtime.status(linked.to_str().unwrap()).unwrap();
    assert!(linked_status.is_managed_worktree);
    assert_eq!(
        linked_status.main_repo_root.as_deref(),
        fixture.repo.canonicalize().unwrap().to_str()
    );
    assert!(runtime.status(standalone.to_str().unwrap()).unwrap().is_git);
    assert!(
        !runtime
            .status(standalone.to_str().unwrap())
            .unwrap()
            .is_managed_worktree
    );
}

#[test]
fn status_diff_and_refresh_cover_git_and_non_git_directories() {
    let fixture = Fixture::new();
    let runtime = LocalCheckout::new(fixture.temp.path().join("managed"));
    let plain = fixture.temp.path().join("plain");
    std::fs::create_dir(&plain).unwrap();
    assert!(!runtime.status(plain.to_str().unwrap()).unwrap().is_git);

    std::fs::write(fixture.repo.join("tracked.txt"), "one\ntwo\n").unwrap();
    std::fs::write(fixture.repo.join("new.txt"), "new\n").unwrap();
    let status = runtime.status(fixture.repo.to_str().unwrap()).unwrap();
    assert!(status.is_git);
    assert!(status.is_dirty.unwrap());
    assert_eq!(status.current_branch.as_deref(), Some("main"));
    assert!(!status.is_managed_worktree);
    runtime.refresh(fixture.repo.to_str().unwrap()).unwrap();

    let diff = runtime
        .diff(
            fixture.repo.to_str().unwrap(),
            &CheckoutDiffCompare {
                mode: CheckoutDiffMode::Uncommitted,
                base_ref: None,
                ignore_whitespace: false,
            },
        )
        .unwrap();
    assert_eq!(
        diff.files
            .iter()
            .map(|file| file.path.as_str())
            .collect::<Vec<_>>(),
        ["new.txt", "tracked.txt"]
    );
    assert_eq!(diff.files[0].additions, 1);
    assert_eq!(diff.files[1].additions, 1);
    assert_eq!(diff.files[1].deletions, 0);
}

#[test]
fn commit_history_and_file_diff_match_paseo_order_and_shape() {
    let fixture = Fixture::new();
    git(&fixture.repo, &["checkout", "-b", "feature"]);
    std::fs::write(fixture.repo.join("tracked.txt"), "feature\n").unwrap();
    git(&fixture.repo, &["add", "tracked.txt"]);
    git(&fixture.repo, &["commit", "-m", "feature subject"]);
    let runtime = LocalCheckout::new(fixture.temp.path().join("managed"));
    let commits = runtime.commits(fixture.repo.to_str().unwrap()).unwrap();
    assert_eq!(commits.base_ref.as_deref(), Some("main"));
    assert_eq!(commits.commits[0].subject, "feature subject");
    assert!(!commits.commits[0].is_on_base);
    assert!(commits.commits.iter().any(|commit| commit.is_on_base));
    let file = runtime
        .commit_file_diff(
            fixture.repo.to_str().unwrap(),
            &commits.commits[0].sha,
            "tracked.txt",
        )
        .unwrap()
        .unwrap();
    assert_eq!(file.path, "tracked.txt");
    assert_eq!(file.additions, 1);
    assert_eq!(file.deletions, 1);
    assert!(
        runtime
            .commit_file_diff(
                fixture.repo.to_str().unwrap(),
                &commits.commits[0].sha,
                "../outside",
            )
            .is_err()
    );
}

#[test]
fn commit_history_marks_remote_commits_and_limits_base_context() {
    let fixture = Fixture::new();
    for index in 1..=11 {
        std::fs::write(fixture.repo.join("base.txt"), format!("{index}\n")).unwrap();
        git(&fixture.repo, &["add", "base.txt"]);
        git(&fixture.repo, &["commit", "-m", &format!("Base {index}")]);
    }
    git(&fixture.repo, &["checkout", "-b", "feature"]);
    std::fs::write(fixture.repo.join("remote.txt"), "remote\n").unwrap();
    git(&fixture.repo, &["add", "remote.txt"]);
    git(&fixture.repo, &["commit", "-m", "Remote feature"]);

    let remote = fixture.temp.path().join("remote.git");
    git(
        fixture.temp.path(),
        &["init", "--bare", "-b", "main", remote.to_str().unwrap()],
    );
    git(
        &fixture.repo,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    git(&fixture.repo, &["push", "-u", "origin", "feature"]);

    std::fs::write(fixture.repo.join("local.txt"), "local\n").unwrap();
    git(&fixture.repo, &["add", "local.txt"]);
    git(&fixture.repo, &["commit", "-m", "Local feature"]);

    let runtime = LocalCheckout::new(fixture.temp.path().join("managed"));
    let commits = runtime.commits(fixture.repo.to_str().unwrap()).unwrap();
    assert_eq!(commits.base_ref.as_deref(), Some("main"));
    assert_eq!(commits.commits.len(), 12);
    assert_eq!(commits.commits[0].subject, "Local feature");
    assert!(!commits.commits[0].is_on_remote);
    assert!(!commits.commits[0].is_on_base);
    assert_eq!(commits.commits[1].subject, "Remote feature");
    assert!(commits.commits[1].is_on_remote);
    assert!(!commits.commits[1].is_on_base);
    assert!(commits.commits[2..].iter().all(|commit| commit.is_on_base));
    assert!(
        commits.commits[2..]
            .iter()
            .all(|commit| commit.is_on_remote)
    );
    assert_eq!(commits.commits[2].subject, "Base 11");
    assert_eq!(commits.commits[11].subject, "Base 2");
}

#[test]
fn commit_history_starts_base_context_at_the_fork_point() {
    let fixture = Fixture::new();
    std::fs::write(fixture.repo.join("shared.txt"), "shared\n").unwrap();
    git(&fixture.repo, &["add", "shared.txt"]);
    git(&fixture.repo, &["commit", "-m", "Shared base"]);
    git(&fixture.repo, &["checkout", "-b", "feature"]);
    std::fs::write(fixture.repo.join("feature.txt"), "feature\n").unwrap();
    git(&fixture.repo, &["add", "feature.txt"]);
    git(&fixture.repo, &["commit", "-m", "Feature work"]);
    git(&fixture.repo, &["checkout", "main"]);
    std::fs::write(fixture.repo.join("newer-base.txt"), "newer\n").unwrap();
    git(&fixture.repo, &["add", "newer-base.txt"]);
    git(&fixture.repo, &["commit", "-m", "Newer base"]);
    git(&fixture.repo, &["checkout", "feature"]);

    let runtime = LocalCheckout::new(fixture.temp.path().join("managed"));
    let commits = runtime.commits(fixture.repo.to_str().unwrap()).unwrap();
    assert_eq!(
        commits
            .commits
            .iter()
            .map(|commit| (commit.subject.as_str(), commit.is_on_base))
            .collect::<Vec<_>>(),
        [
            ("Feature work", false),
            ("Shared base", true),
            ("base subject", true),
        ]
    );
}

#[test]
fn commit_history_classifies_merge_rename_modify_and_delete_files() {
    let fixture = Fixture::new();
    git(&fixture.repo, &["checkout", "-b", "feature"]);
    std::fs::write(fixture.repo.join("feature.txt"), "feature\n").unwrap();
    git(&fixture.repo, &["add", "feature.txt"]);
    git(&fixture.repo, &["commit", "-m", "Add feature"]);
    git(&fixture.repo, &["checkout", "main"]);
    std::fs::write(fixture.repo.join("main.txt"), "main\n").unwrap();
    git(&fixture.repo, &["add", "main.txt"]);
    git(&fixture.repo, &["commit", "-m", "Advance main"]);
    git(
        &fixture.repo,
        &["merge", "--no-ff", "feature", "-m", "Merge feature"],
    );
    std::fs::write(fixture.repo.join("original.txt"), "content\n").unwrap();
    git(&fixture.repo, &["add", "original.txt"]);
    git(&fixture.repo, &["commit", "-m", "Add original"]);
    git(&fixture.repo, &["mv", "original.txt", "renamed.txt"]);
    git(&fixture.repo, &["commit", "-m", "Rename file"]);
    std::fs::write(fixture.repo.join("tracked.txt"), "one\nmore\n").unwrap();
    git(&fixture.repo, &["add", "tracked.txt"]);
    git(&fixture.repo, &["commit", "-m", "Edit tracked"]);
    git(&fixture.repo, &["rm", "tracked.txt"]);
    git(&fixture.repo, &["commit", "-m", "Delete tracked"]);

    let runtime = LocalCheckout::new(fixture.temp.path().join("managed"));
    let commits = runtime.commits(fixture.repo.to_str().unwrap()).unwrap();
    assert_eq!(commits.base_ref, None);
    assert_eq!(
        commits.commits[0].files[0].status,
        Some(CheckoutCommitFileStatus::Deleted)
    );
    assert_eq!(
        commits.commits[1].files[0].status,
        Some(CheckoutCommitFileStatus::Modified)
    );
    assert_eq!(commits.commits[2].files[0].path, "renamed.txt");
    assert_eq!(
        commits.commits[2].files[0].status,
        Some(CheckoutCommitFileStatus::Renamed)
    );
    let merge = commits
        .commits
        .iter()
        .find(|commit| commit.subject == "Merge feature")
        .unwrap();
    assert_eq!(merge.files[0].path, "feature.txt");
    assert_eq!(merge.files[0].status, Some(CheckoutCommitFileStatus::Added));
}

#[test]
fn base_diff_excludes_uncommitted_changes() {
    let fixture = Fixture::new();
    git(&fixture.repo, &["checkout", "-b", "feature"]);
    std::fs::write(fixture.repo.join("committed.txt"), "committed\n").unwrap();
    git(&fixture.repo, &["add", "committed.txt"]);
    git(&fixture.repo, &["commit", "-m", "Committed feature"]);
    std::fs::write(fixture.repo.join("working.txt"), "working\n").unwrap();

    let runtime = LocalCheckout::new(fixture.temp.path().join("managed"));
    let diff = runtime
        .diff(
            fixture.repo.to_str().unwrap(),
            &CheckoutDiffCompare {
                mode: CheckoutDiffMode::Base,
                base_ref: Some("main".to_owned()),
                ignore_whitespace: false,
            },
        )
        .unwrap();
    assert_eq!(
        diff.files
            .iter()
            .map(|file| file.path.as_str())
            .collect::<Vec<_>>(),
        ["committed.txt"]
    );
}

#[test]
fn branch_validation_suggestions_switch_and_rename_match_paseo() {
    let fixture = Fixture::new();
    git(&fixture.repo, &["branch", "feature"]);
    let remote = fixture.temp.path().join("remote.git");
    git(
        fixture.temp.path(),
        &["init", "--bare", "-b", "main", remote.to_str().unwrap()],
    );
    git(
        &fixture.repo,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    git(&fixture.repo, &["branch", "remote-only"]);
    git(
        &fixture.repo,
        &["push", "origin", "main", "feature", "remote-only"],
    );
    git(&fixture.repo, &["branch", "-D", "remote-only"]);

    let runtime = LocalCheckout::new(fixture.temp.path().join("managed"));
    assert_eq!(
        runtime
            .validate_branch(fixture.repo.to_str().unwrap(), "feature")
            .unwrap(),
        CheckoutBranchResolution::Local("feature".to_owned())
    );
    assert_eq!(
        runtime
            .validate_branch(fixture.repo.to_str().unwrap(), "origin/remote-only")
            .unwrap(),
        CheckoutBranchResolution::RemoteOnly {
            name: "remote-only".to_owned(),
            remote_ref: "origin/remote-only".to_owned(),
        }
    );
    assert_eq!(
        runtime
            .validate_branch(fixture.repo.to_str().unwrap(), "missing")
            .unwrap(),
        CheckoutBranchResolution::NotFound
    );
    assert!(
        runtime
            .validate_branch(fixture.repo.to_str().unwrap(), "bad ref!")
            .is_err()
    );

    let suggestions = runtime
        .branch_suggestions(fixture.repo.to_str().unwrap(), Some("origin/remote"), 10)
        .unwrap();
    assert_eq!(suggestions.len(), 1);
    assert_eq!(suggestions[0].name, "remote-only");
    assert!(!suggestions[0].has_local);
    assert!(suggestions[0].has_remote);
    let suggestions = runtime
        .branch_suggestions(fixture.repo.to_str().unwrap(), Some("feature"), 10)
        .unwrap();
    assert_eq!(suggestions[0].local_ahead, Some(0));
    assert_eq!(suggestions[0].local_behind, Some(0));

    assert_eq!(
        runtime
            .switch_branch(fixture.repo.to_str().unwrap(), "remote-only")
            .unwrap(),
        CheckoutBranchSource::Remote
    );
    assert_eq!(
        runtime
            .rename_branch(fixture.repo.to_str().unwrap(), "renamed")
            .unwrap(),
        "renamed"
    );
    let rename_error = runtime
        .rename_branch(fixture.repo.to_str().unwrap(), "Bad_Name")
        .unwrap_err();
    assert_eq!(rename_error.kind, CheckoutFailureKind::Unknown);
    std::fs::write(fixture.repo.join("tracked.txt"), "dirty\n").unwrap();
    assert!(
        runtime
            .switch_branch(fixture.repo.to_str().unwrap(), "main")
            .is_err()
    );
}

#[test]
fn commit_discard_and_stash_mutations_match_paseo() {
    let fixture = Fixture::new();
    let runtime = LocalCheckout::new(fixture.temp.path().join("managed"));
    std::fs::write(fixture.repo.join("tracked.txt"), "committed\n").unwrap();
    std::fs::write(fixture.repo.join("added.txt"), "added\n").unwrap();
    runtime
        .commit(fixture.repo.to_str().unwrap(), "  mutation commit  ", true)
        .unwrap();
    assert_eq!(
        git_output(&fixture.repo, &["log", "-1", "--format=%s"]),
        "mutation commit"
    );

    std::fs::write(fixture.repo.join("tracked.txt"), "discard me\n").unwrap();
    std::fs::write(fixture.repo.join("untracked.txt"), "remove me\n").unwrap();
    runtime
        .discard_changes(
            fixture.repo.to_str().unwrap(),
            &["tracked.txt".to_owned(), "untracked.txt".to_owned()],
        )
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(fixture.repo.join("tracked.txt")).unwrap(),
        "committed\n"
    );
    assert!(!fixture.repo.join("untracked.txt").exists());

    std::fs::write(fixture.repo.join("tracked.txt"), "stash me\n").unwrap();
    std::fs::write(fixture.repo.join("stash-untracked.txt"), "stash me too\n").unwrap();
    runtime
        .stash_save(fixture.repo.to_str().unwrap(), Some(" feature "))
        .unwrap();
    let entries = runtime
        .stashes(fixture.repo.to_str().unwrap(), true)
        .unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].branch.as_deref(), Some("feature"));
    assert!(entries[0].is_paseo);
    runtime
        .stash_pop(fixture.repo.to_str().unwrap(), 0)
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(fixture.repo.join("tracked.txt")).unwrap(),
        "stash me\n"
    );
    assert!(fixture.repo.join("stash-untracked.txt").exists());
    runtime
        .discard_changes(
            fixture.repo.to_str().unwrap(),
            &["tracked.txt".to_owned(), "stash-untracked.txt".to_owned()],
        )
        .unwrap();
    std::fs::write(fixture.repo.join("tracked.txt"), "ordinary stash\n").unwrap();
    git(&fixture.repo, &["stash", "push", "-m", "ordinary"]);
    assert!(
        runtime
            .stashes(fixture.repo.to_str().unwrap(), true)
            .unwrap()
            .is_empty()
    );
    let entries = runtime
        .stashes(fixture.repo.to_str().unwrap(), false)
        .unwrap();
    assert_eq!(entries.len(), 1);
    assert!(!entries[0].is_paseo);
}

#[test]
fn merge_from_base_and_merge_to_base_follow_paseo_direction() {
    let fixture = Fixture::new();
    git(&fixture.repo, &["checkout", "-b", "feature"]);
    std::fs::write(fixture.repo.join("feature.txt"), "feature\n").unwrap();
    git(&fixture.repo, &["add", "feature.txt"]);
    git(&fixture.repo, &["commit", "-m", "feature change"]);
    git(&fixture.repo, &["checkout", "main"]);
    std::fs::write(fixture.repo.join("base.txt"), "base\n").unwrap();
    git(&fixture.repo, &["add", "base.txt"]);
    git(&fixture.repo, &["commit", "-m", "base change"]);
    git(&fixture.repo, &["checkout", "feature"]);

    let runtime = LocalCheckout::new(fixture.temp.path().join("managed"));
    runtime
        .merge_from_base(fixture.repo.to_str().unwrap(), Some("main"), true)
        .unwrap();
    assert!(fixture.repo.join("base.txt").exists());
    std::fs::write(fixture.repo.join("after-merge.txt"), "feature\n").unwrap();
    git(&fixture.repo, &["add", "after-merge.txt"]);
    git(&fixture.repo, &["commit", "-m", "after merge"]);
    runtime
        .merge_to_base(
            fixture.repo.to_str().unwrap(),
            Some("main"),
            CheckoutMergeStrategy::Merge,
            true,
        )
        .unwrap();
    assert_eq!(
        git_output(&fixture.repo, &["symbolic-ref", "--short", "HEAD"]),
        "feature"
    );
    assert_eq!(
        git_output(
            &fixture.repo,
            &["merge-base", "--is-ancestor", "feature", "main"]
        ),
        ""
    );
}

#[test]
fn merge_conflicts_are_aborted_and_categorized() {
    let fixture = Fixture::new();
    git(&fixture.repo, &["checkout", "-b", "feature"]);
    std::fs::write(fixture.repo.join("tracked.txt"), "feature\n").unwrap();
    git(&fixture.repo, &["add", "tracked.txt"]);
    git(&fixture.repo, &["commit", "-m", "feature conflict"]);
    git(&fixture.repo, &["checkout", "main"]);
    std::fs::write(fixture.repo.join("tracked.txt"), "main\n").unwrap();
    git(&fixture.repo, &["add", "tracked.txt"]);
    git(&fixture.repo, &["commit", "-m", "main conflict"]);
    git(&fixture.repo, &["checkout", "feature"]);

    let runtime = LocalCheckout::new(fixture.temp.path().join("managed"));
    let error = runtime
        .merge_from_base(fixture.repo.to_str().unwrap(), Some("main"), true)
        .unwrap_err();
    assert_eq!(error.kind, CheckoutFailureKind::MergeConflict);
    assert!(git_output(&fixture.repo, &["status", "--porcelain"]).is_empty());
}

#[test]
fn squash_merge_to_base_restores_feature_and_creates_one_base_commit() {
    let fixture = Fixture::new();
    git(&fixture.repo, &["checkout", "-b", "feature"]);
    for (path, message) in [("one.txt", "one"), ("two.txt", "two")] {
        std::fs::write(fixture.repo.join(path), format!("{path}\n")).unwrap();
        git(&fixture.repo, &["add", path]);
        git(&fixture.repo, &["commit", "-m", message]);
    }
    let runtime = LocalCheckout::new(fixture.temp.path().join("managed"));
    runtime
        .merge_to_base(
            fixture.repo.to_str().unwrap(),
            Some("main"),
            CheckoutMergeStrategy::Squash,
            true,
        )
        .unwrap();
    assert_eq!(
        git_output(&fixture.repo, &["symbolic-ref", "--short", "HEAD"]),
        "feature"
    );
    assert_eq!(
        git_output(&fixture.repo, &["log", "-1", "--format=%s", "main"]),
        "Squash merge feature into main"
    );
    assert_eq!(
        git_output(&fixture.repo, &["diff", "--name-only", "main", "feature"]),
        ""
    );
}

#[test]
fn merge_to_base_mutates_an_existing_base_worktree() {
    let fixture = Fixture::new();
    let feature = fixture.temp.path().join("feature-worktree");
    git(
        &fixture.repo,
        &[
            "worktree",
            "add",
            "-b",
            "feature",
            feature.to_str().unwrap(),
        ],
    );
    std::fs::write(feature.join("feature.txt"), "feature\n").unwrap();
    git(&feature, &["add", "feature.txt"]);
    git(&feature, &["commit", "-m", "linked feature"]);
    let runtime = LocalCheckout::new(fixture.temp.path().join("managed"));
    runtime
        .merge_to_base(
            feature.to_str().unwrap(),
            Some("main"),
            CheckoutMergeStrategy::Merge,
            true,
        )
        .unwrap();
    assert_eq!(
        git_output(&feature, &["symbolic-ref", "--short", "HEAD"]),
        "feature"
    );
    assert_eq!(
        git_output(&fixture.repo, &["symbolic-ref", "--short", "HEAD"]),
        "main"
    );
    assert!(fixture.repo.join("feature.txt").exists());
}

#[test]
fn configured_push_remote_and_head_refspec_are_honored() {
    let fixture = Fixture::new();
    let remote = fixture.temp.path().join("publish.git");
    git(
        fixture.temp.path(),
        &["init", "--bare", "-b", "main", remote.to_str().unwrap()],
    );
    git(
        &fixture.repo,
        &["remote", "add", "publish", remote.to_str().unwrap()],
    );
    git(
        &fixture.repo,
        &["config", "branch.main.pushRemote", "publish"],
    );
    git(
        &fixture.repo,
        &["config", "remote.publish.push", "HEAD:refs/heads/review"],
    );
    let runtime = LocalCheckout::new(fixture.temp.path().join("managed"));
    runtime.push(fixture.repo.to_str().unwrap()).unwrap();
    assert_eq!(
        git_output(&remote, &["rev-parse", "refs/heads/review"]),
        git_output(&fixture.repo, &["rev-parse", "HEAD"])
    );
}

#[test]
fn pull_and_push_use_local_origin_without_network() {
    let fixture = Fixture::new();
    let runtime = LocalCheckout::new(fixture.temp.path().join("managed"));
    assert_eq!(
        runtime
            .pull(fixture.repo.to_str().unwrap())
            .unwrap_err()
            .message,
        "Remote 'origin' is not configured."
    );
    let remote = fixture.temp.path().join("remote.git");
    git(
        fixture.temp.path(),
        &["init", "--bare", "-b", "main", remote.to_str().unwrap()],
    );
    git(
        &fixture.repo,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    git(&fixture.repo, &["push", "-u", "origin", "main"]);
    let peer = fixture.temp.path().join("peer");
    git(
        fixture.temp.path(),
        &["clone", remote.to_str().unwrap(), peer.to_str().unwrap()],
    );
    git(&peer, &["config", "user.email", "peer@example.test"]);
    git(&peer, &["config", "user.name", "Peer"]);
    std::fs::write(peer.join("peer.txt"), "peer\n").unwrap();
    git(&peer, &["add", "peer.txt"]);
    git(&peer, &["commit", "-m", "peer change"]);
    git(&peer, &["push"]);

    runtime.pull(fixture.repo.to_str().unwrap()).unwrap();
    assert!(fixture.repo.join("peer.txt").exists());
    std::fs::write(fixture.repo.join("local.txt"), "local\n").unwrap();
    runtime
        .commit(fixture.repo.to_str().unwrap(), "local change", true)
        .unwrap();
    runtime.push(fixture.repo.to_str().unwrap()).unwrap();
    assert_eq!(
        git_output(&fixture.repo, &["rev-parse", "HEAD"]),
        git_output(&fixture.repo, &["rev-parse", "origin/main"])
    );
}

struct Fixture {
    temp: TempDir,
    repo: std::path::PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        git(&repo, &["init", "-b", "main"]);
        git(&repo, &["config", "user.email", "server@example.test"]);
        git(&repo, &["config", "user.name", "Server Test"]);
        std::fs::write(repo.join("tracked.txt"), "one\n").unwrap();
        git(&repo, &["add", "tracked.txt"]);
        git(&repo, &["commit", "-m", "base subject"]);
        Self { temp, repo }
    }
}

fn git(cwd: &Path, arguments: &[&str]) {
    let output = Command::new("git")
        .args(arguments)
        .current_dir(cwd)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {:?}: {}",
        arguments,
        String::from_utf8_lossy(&output.stderr)
    );
}

fn git_output(cwd: &Path, arguments: &[&str]) -> String {
    let output = Command::new("git")
        .args(arguments)
        .current_dir(cwd)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {:?}: {}",
        arguments,
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}
