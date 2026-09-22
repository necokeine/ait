use std::path::Path;
use std::process::Command;

use server_ports::checkout::{
    CheckoutCommitFileStatus, CheckoutDiffCompare, CheckoutDiffMode, CheckoutRuntime,
};
use tempfile::TempDir;

use super::{LocalCheckout, is_below};

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
