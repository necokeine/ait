use std::process::Command;

use super::*;

struct Fixture {
    root: tempfile::TempDir,
    repository: PathBuf,
    managed_root: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().expect("temp directory");
        let repository = root.path().join("repository");
        let managed_root = root.path().join("managed");
        std::fs::create_dir(&repository).expect("repository directory");
        run(&repository, &["init", "--quiet", "--initial-branch=main"]);
        std::fs::write(repository.join("README.md"), "baseline\n").expect("write baseline");
        run(&repository, &["add", "README.md"]);
        run(
            &repository,
            &[
                "-c",
                "user.name=Server Test",
                "-c",
                "user.email=server@example.invalid",
                "commit",
                "--quiet",
                "-m",
                "baseline",
            ],
        );
        Self {
            root,
            repository,
            managed_root,
        }
    }

    fn adapter(&self) -> LocalManagedWorktrees {
        LocalManagedWorktrees::new(self.managed_root.clone())
    }
}

#[test]
fn creates_from_nested_cwd_maps_directory_and_seeds_untracked_config() {
    let fixture = Fixture::new();
    let nested = fixture.repository.join("packages/app");
    std::fs::create_dir_all(&nested).expect("nested source");
    std::fs::write(nested.join("tracked.txt"), "tracked\n").expect("tracked source");
    run(&fixture.repository, &["add", "."]);
    run(
        &fixture.repository,
        &[
            "-c",
            "user.name=Server Test",
            "-c",
            "user.email=server@example.invalid",
            "commit",
            "--quiet",
            "-m",
            "nested",
        ],
    );
    std::fs::write(nested.join("paseo.json"), "{\"scripts\":{}}\n").expect("untracked config");

    let adapter = fixture.adapter();
    let created = adapter
        .create(&ManagedWorktreeCreate {
            cwd: path(&nested),
            slug: "feature-review".to_owned(),
            mode: WorktreeCreateMode::BranchOff {
                base_ref: Some("main".to_owned()),
                branch_name: "feature-review".to_owned(),
            },
        })
        .expect("create worktree");

    assert_eq!(created.branch_name, "feature-review");
    assert_eq!(
        Path::new(&created.workspace_cwd)
            .strip_prefix(&created.worktree_path)
            .expect("mapped cwd"),
        Path::new("packages/app")
    );
    assert_eq!(
        std::fs::read_to_string(Path::new(&created.workspace_cwd).join("paseo.json"))
            .expect("seeded config"),
        "{\"scripts\":{}}\n"
    );
    assert_eq!(branch(Path::new(&created.worktree_path)), "feature-review");
    assert_eq!(
        created.comparison_base_ref.as_deref(),
        Some("refs/heads/main")
    );
}

#[test]
fn lists_only_managed_worktrees_with_branch_and_head() {
    let fixture = Fixture::new();
    let adapter = fixture.adapter();
    let created = adapter
        .create(&ManagedWorktreeCreate {
            cwd: path(&fixture.repository),
            slug: "managed".to_owned(),
            mode: WorktreeCreateMode::BranchOff {
                base_ref: None,
                branch_name: "managed".to_owned(),
            },
        })
        .expect("managed worktree");
    let external = fixture.root.path().join("external");
    run(
        &fixture.repository,
        &[
            "worktree",
            "add",
            "--quiet",
            "--detach",
            external.to_str().expect("external path"),
        ],
    );

    let listed = adapter
        .list(fixture.repository.to_str().expect("repository path"))
        .expect("list managed worktrees");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].path, created.worktree_path);
    assert_eq!(listed[0].branch_name.as_deref(), Some("managed"));
    assert!(listed[0].head.as_ref().is_some_and(|head| head.len() == 40));
    assert_ne!(listed[0].created_at, "1970-01-01T00:00:00.000Z");
}

#[test]
fn branch_and_path_collisions_receive_paseo_suffixes() {
    let fixture = Fixture::new();
    let adapter = fixture.adapter();
    let first = adapter
        .create(&ManagedWorktreeCreate {
            cwd: path(&fixture.repository),
            slug: "feature".to_owned(),
            mode: WorktreeCreateMode::BranchOff {
                base_ref: Some("main".to_owned()),
                branch_name: "feature".to_owned(),
            },
        })
        .expect("first worktree");
    let second = adapter
        .create(&ManagedWorktreeCreate {
            cwd: path(&fixture.repository),
            slug: "feature".to_owned(),
            mode: WorktreeCreateMode::BranchOff {
                base_ref: Some("main".to_owned()),
                branch_name: "feature".to_owned(),
            },
        })
        .expect("second worktree");
    assert_eq!(first.branch_name, "feature");
    assert_eq!(second.branch_name, "feature-1");
    assert!(second.worktree_path.ends_with("feature-1"));
}

#[test]
fn checkout_unknown_branch_has_stable_error_and_checked_out_branch_is_copied() {
    let fixture = Fixture::new();
    let adapter = fixture.adapter();
    let unknown = adapter.create(&ManagedWorktreeCreate {
        cwd: path(&fixture.repository),
        slug: "missing".to_owned(),
        mode: WorktreeCreateMode::Checkout {
            branch_name: "missing-branch".to_owned(),
        },
    });
    assert_eq!(
        unknown,
        Err(WorktreeError::UnknownBranch("missing-branch".to_owned()))
    );

    let copied = adapter
        .create(&ManagedWorktreeCreate {
            cwd: path(&fixture.repository),
            slug: "main-copy".to_owned(),
            mode: WorktreeCreateMode::Checkout {
                branch_name: "main".to_owned(),
            },
        })
        .expect("copy checked-out branch");
    assert_eq!(copied.branch_name, "main-1");
}

#[test]
fn ownership_accepts_descendants_and_removal_rejects_external_paths() {
    let fixture = Fixture::new();
    let adapter = fixture.adapter();
    let created = adapter
        .create(&ManagedWorktreeCreate {
            cwd: path(&fixture.repository),
            slug: "archive".to_owned(),
            mode: WorktreeCreateMode::BranchOff {
                base_ref: None,
                branch_name: "archive".to_owned(),
            },
        })
        .expect("create archive target");
    let descendant = Path::new(&created.worktree_path).join("child");
    std::fs::create_dir(&descendant).expect("descendant");
    let owned = adapter
        .owned(descendant.to_str().expect("descendant path"))
        .expect("ownership");
    assert_eq!(owned.path, created.worktree_path);
    adapter.remove(&owned).expect("remove managed worktree");
    assert!(!Path::new(&created.worktree_path).exists());
    assert_eq!(
        adapter.owned(fixture.repository.to_str().expect("repository path")),
        Err(WorktreeError::NotAllowed)
    );
}

#[test]
fn repositories_receive_distinct_hashed_roots() {
    let first = Fixture::new();
    let second = Fixture::new();
    let shared_root = first.root.path().join("shared");
    let adapter = LocalManagedWorktrees::new(shared_root);
    let first_path = adapter
        .path_for_slug(
            first.repository.to_str().expect("first repository"),
            "topic",
        )
        .expect("first path");
    let second_path = adapter
        .path_for_slug(
            second.repository.to_str().expect("second repository"),
            "topic",
        )
        .expect("second path");
    assert_ne!(
        Path::new(&first_path).parent(),
        Path::new(&second_path).parent()
    );
}

fn run(root: &Path, arguments: &[&str]) {
    let output = Command::new("git")
        .args(arguments)
        .current_dir(root)
        .output()
        .expect("run git");
    assert!(
        output.status.success(),
        "git {arguments:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn branch(root: &Path) -> String {
    let output = Command::new("git")
        .args(["branch", "--show-current"])
        .current_dir(root)
        .output()
        .expect("read branch");
    assert!(output.status.success());
    String::from_utf8(output.stdout)
        .expect("UTF-8 branch")
        .trim()
        .to_owned()
}

fn path(path: &Path) -> String {
    path.to_str().expect("UTF-8 path").to_owned()
}
