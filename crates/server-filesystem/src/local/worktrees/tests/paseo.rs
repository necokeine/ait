//! Paseo worktree ownership, source mapping, branch selection and recovery contracts.

use super::*;

fn create_input(fixture: &Fixture, slug: &str) -> ManagedWorktreeCreate {
    ManagedWorktreeCreate {
        cwd: path(&fixture.repository),
        slug: slug.to_owned(),
        mode: WorktreeCreateMode::BranchOff {
            base_ref: Some("main".to_owned()),
            branch_name: slug.to_owned(),
        },
    }
}

fn create(fixture: &Fixture, slug: &str) -> CreatedManagedWorktree {
    fixture
        .adapter()
        .create(&create_input(fixture, slug))
        .unwrap()
}

fn commit(root: &Path, message: &str) {
    run(root, &["add", "."]);
    run(
        root,
        &[
            "-c",
            "user.name=Server Test",
            "-c",
            "user.email=server@example.invalid",
            "commit",
            "--quiet",
            "-m",
            message,
        ],
    );
}

fn head(root: &Path) -> String {
    let output = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(root)
        .output()
        .unwrap();
    assert!(output.status.success());
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

fn archived(fixture: &Fixture, slug: &str) -> ArchivedWorktreeRestore {
    let created = create(fixture, slug);
    fixture
        .adapter()
        .remove(&OwnedWorktree {
            path: created.worktree_path.clone(),
            repo_root: Some(created.repo_root.clone()),
        })
        .unwrap();
    ArchivedWorktreeRestore {
        source_repo_root: created.repo_root,
        previous_worktree_root: created.worktree_path.clone(),
        workspace_cwd: created.worktree_path,
        branch: created.branch_name,
        base_ref: created.comparison_base_ref,
    }
}

#[test]
fn ownership_survives_a_missing_git_administration_pointer() {
    let fixture = Fixture::new();
    let created = create(&fixture, "orphan");
    std::fs::remove_file(Path::new(&created.worktree_path).join(".git")).unwrap();
    let owned = fixture.adapter().owned(&created.worktree_path).unwrap();
    assert_eq!(owned.path, created.worktree_path);
    assert!(owned.repo_root.is_none());
}

#[test]
fn removal_succeeds_after_git_administration_was_already_deleted() {
    let fixture = Fixture::new();
    let created = create(&fixture, "orphan");
    let pointer = std::fs::read_to_string(Path::new(&created.worktree_path).join(".git")).unwrap();
    let admin = pointer.trim().strip_prefix("gitdir: ").unwrap();
    std::fs::remove_dir_all(admin).unwrap();
    fixture
        .adapter()
        .remove(&OwnedWorktree {
            path: created.worktree_path.clone(),
            repo_root: Some(created.repo_root),
        })
        .unwrap();
    assert!(!Path::new(&created.worktree_path).exists());
    assert!(fixture.repository.join("README.md").is_file());
}

#[test]
fn removing_an_absent_owned_worktree_is_idempotent() {
    let fixture = Fixture::new();
    let request = archived(&fixture, "already-removed");
    let owned = fixture
        .adapter()
        .owned(&request.previous_worktree_root)
        .unwrap();
    fixture.adapter().remove(&owned).unwrap();
    fixture.adapter().remove(&owned).unwrap();
    assert!(!Path::new(&owned.path).exists());
    assert!(fixture.repository.join("README.md").is_file());
}

#[test]
fn removal_without_the_original_repository_keeps_unrelated_managed_worktrees() {
    let fixture = Fixture::new();
    let first = create(&fixture, "first");
    let sibling = create(&fixture, "sibling");
    std::fs::remove_dir_all(&fixture.repository).unwrap();
    fixture
        .adapter()
        .remove(&OwnedWorktree {
            path: first.worktree_path.clone(),
            repo_root: Some(first.repo_root),
        })
        .unwrap();
    assert!(!Path::new(&first.worktree_path).exists());
    assert!(
        Path::new(&sibling.worktree_path)
            .join("README.md")
            .is_file()
    );
}

#[test]
fn ownership_rejects_the_managed_root_hash_directory_and_outside_paths() {
    let fixture = Fixture::new();
    let created = create(&fixture, "owned");
    let hash_root = Path::new(&created.worktree_path).parent().unwrap();
    for target in [&fixture.managed_root, hash_root, fixture.root.path()] {
        assert_eq!(
            fixture.adapter().owned(&path(target)),
            Err(WorktreeError::NotAllowed)
        );
    }
    assert!(
        Path::new(&created.worktree_path)
            .join("README.md")
            .is_file()
    );
}

#[test]
fn containment_uses_directory_boundaries_and_normalizes_parent_components() {
    let fixture = Fixture::new();
    let adapter = fixture.adapter();
    let root = path(&fixture.repository);
    assert!(adapter.contains(&root, &root));
    assert!(adapter.contains(&root, &format!("{root}/child/../nested")));
    assert!(!adapter.contains(&root, &format!("{root}-sibling/file")));
    assert!(!adapter.contains(&root, &format!("{root}/../outside")));
}

#[cfg(unix)]
#[test]
fn missing_descendants_compare_equally_through_a_realpath_alias() {
    let fixture = Fixture::new();
    let alias = fixture.root.path().join("source-alias");
    std::os::unix::fs::symlink(&fixture.repository, &alias).unwrap();
    let real = fixture.repository.canonicalize().unwrap();
    let adapter = fixture.adapter();
    assert!(adapter.contains(&path(&real), &path(&alias.join("missing/nested"))));
    assert!(adapter.contains(&path(&alias), &path(&real.join("missing/nested"))));
}

#[cfg(unix)]
#[test]
fn missing_descendants_of_an_external_symlink_never_count_as_contained() {
    let fixture = Fixture::new();
    let outside = fixture.root.path().join("outside");
    std::fs::create_dir(&outside).unwrap();
    let link = fixture.repository.join("escape");
    std::os::unix::fs::symlink(&outside, &link).unwrap();
    let adapter = fixture.adapter();
    let root = path(&fixture.repository);
    assert!(!adapter.contains(&root, &path(&link.join("missing"))));
    assert!(!adapter.contains(&root, &path(&link.join("../missing"))));
    assert!(!adapter.contains(&root, &path(&link.join("missing/../another"))));
}

#[test]
fn worktree_sources_distinguish_checkout_root_from_shared_git_directory() {
    let fixture = Fixture::new();
    let created = create(&fixture, "source");
    let repository = LocalManagedWorktrees::repository(&created.worktree_path).unwrap();
    assert_eq!(
        repository.repo_root,
        fixture.repository.canonicalize().unwrap()
    );
    assert_eq!(repository.source_cwd, Path::new(&created.worktree_path));
    assert!(repository.relative_cwd.as_os_str().is_empty());
}

#[test]
fn tracked_project_configuration_wins_over_uncommitted_source_edits() {
    let fixture = Fixture::new();
    std::fs::write(
        fixture.repository.join("paseo.json"),
        "{\"committed\":true}\n",
    )
    .unwrap();
    commit(&fixture.repository, "configuration");
    std::fs::write(fixture.repository.join("paseo.json"), "{\"local\":true}\n").unwrap();
    let created = create(&fixture, "clean");
    assert_eq!(
        std::fs::read_to_string(Path::new(&created.worktree_path).join("paseo.json")).unwrap(),
        "{\"committed\":true}\n"
    );
    let status = Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(&created.worktree_path)
        .output()
        .unwrap();
    assert!(status.status.success());
    assert!(status.stdout.is_empty());
}

#[test]
fn nested_cwd_in_an_existing_worktree_maps_into_the_next_checkout() {
    let fixture = Fixture::new();
    std::fs::create_dir_all(fixture.repository.join("packages/app")).unwrap();
    std::fs::write(fixture.repository.join("packages/app/README.md"), "app").unwrap();
    commit(&fixture.repository, "nested");
    let source = create(&fixture, "source");
    let mut request = create_input(&fixture, "next");
    request.cwd = path(&Path::new(&source.worktree_path).join("packages/app"));
    let next = fixture.adapter().create(&request).unwrap();
    assert_eq!(
        Path::new(&next.workspace_cwd),
        Path::new(&next.worktree_path).join("packages/app")
    );
    assert_eq!(next.repo_root, source.repo_root);
    assert_eq!(
        std::fs::read_to_string(Path::new(&next.workspace_cwd).join("README.md")).unwrap(),
        "app"
    );
}

#[test]
fn selected_ref_missing_the_source_subdirectory_removes_the_new_worktree() {
    let fixture = Fixture::new();
    let original = head(&fixture.repository);
    std::fs::create_dir_all(fixture.repository.join("packages/app")).unwrap();
    std::fs::write(fixture.repository.join("packages/app/README.md"), "app").unwrap();
    commit(&fixture.repository, "nested");
    let mut request = create_input(&fixture, "missing-directory");
    request.cwd = path(&fixture.repository.join("packages/app"));
    request.mode = WorktreeCreateMode::BranchOff {
        base_ref: Some(original),
        branch_name: "missing-directory".to_owned(),
    };
    assert!(matches!(
        fixture.adapter().create(&request),
        Err(WorktreeError::Invalid(_))
    ));
    assert!(
        fixture
            .adapter()
            .list(&path(&fixture.repository))
            .unwrap()
            .is_empty()
    );
    let expected = fixture
        .adapter()
        .path_for_slug(&path(&fixture.repository), &request.slug)
        .unwrap();
    assert!(!Path::new(&expected).exists());
}

#[cfg(unix)]
#[test]
fn untracked_symlink_configuration_fails_without_retaining_a_partial_worktree() {
    let fixture = Fixture::new();
    let external = fixture.root.path().join("outside.json");
    std::fs::write(&external, "external").unwrap();
    std::os::unix::fs::symlink(&external, fixture.repository.join("paseo.json")).unwrap();
    assert!(matches!(
        fixture.adapter().create(&create_input(&fixture, "symlink")),
        Err(WorktreeError::Io(_))
    ));
    assert!(
        fixture
            .adapter()
            .list(&path(&fixture.repository))
            .unwrap()
            .is_empty()
    );
    assert_eq!(std::fs::read_to_string(external).unwrap(), "external");
}

#[cfg(unix)]
#[test]
fn selected_ref_dangling_configuration_symlink_is_never_overwritten() {
    let fixture = Fixture::new();
    let config = fixture.repository.join("paseo.json");
    std::os::unix::fs::symlink("missing-config.json", &config).unwrap();
    commit(&fixture.repository, "symlink in selected ref");
    std::fs::remove_file(&config).unwrap();
    std::fs::write(&config, "local source config").unwrap();
    let created = create(&fixture, "preserved");
    let target = Path::new(&created.worktree_path).join("paseo.json");
    assert!(target.symlink_metadata().unwrap().file_type().is_symlink());
    assert_eq!(
        std::fs::read_link(target).unwrap(),
        Path::new("missing-config.json")
    );
}

#[test]
fn managed_detached_worktrees_keep_their_head_without_inventing_a_branch() {
    let fixture = Fixture::new();
    let destination = fixture
        .adapter()
        .path_for_slug(&path(&fixture.repository), "detached")
        .unwrap();
    run(
        &fixture.repository,
        &["worktree", "add", "--quiet", "--detach", &destination],
    );
    let listed = fixture.adapter().list(&path(&fixture.repository)).unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].branch_name, None);
    assert_eq!(
        listed[0].head.as_deref(),
        Some(head(&fixture.repository).as_str())
    );
}

#[test]
fn checkout_preserves_valid_uppercase_and_dotted_branch_names() {
    let fixture = Fixture::new();
    run(&fixture.repository, &["branch", "Release/Version.2"]);
    let mut request = create_input(&fixture, "release");
    request.mode = WorktreeCreateMode::Checkout {
        branch_name: "Release/Version.2".to_owned(),
    };
    let created = fixture.adapter().create(&request).unwrap();
    assert_eq!(created.branch_name, "Release/Version.2");
    assert_eq!(
        branch(Path::new(&created.worktree_path)),
        "Release/Version.2"
    );
    assert!(created.comparison_base_ref.is_none());
}

#[test]
fn checkout_fetches_a_remote_only_branch_from_a_local_fixture_remote() {
    let fixture = Fixture::new();
    let remote = fixture.root.path().join("remote.git");
    run(
        fixture.root.path(),
        &[
            "clone",
            "--quiet",
            "--bare",
            &path(&fixture.repository),
            &path(&remote),
        ],
    );
    run(&remote, &["branch", "remote-only"]);
    run(
        &fixture.repository,
        &["remote", "add", "origin", &path(&remote)],
    );
    let mut request = create_input(&fixture, "fetched");
    request.mode = WorktreeCreateMode::Checkout {
        branch_name: "remote-only".to_owned(),
    };
    let created = fixture.adapter().create(&request).unwrap();
    assert_eq!(created.branch_name, "remote-only");
    assert_eq!(
        head(Path::new(&created.worktree_path)),
        head(&fixture.repository)
    );
    assert_eq!(branch(&fixture.repository), "main");
}

#[test]
fn explicitly_selected_local_and_origin_refs_keep_distinct_starting_commits() {
    let fixture = Fixture::new();
    let old = head(&fixture.repository);
    run(
        &fixture.repository,
        &["update-ref", "refs/remotes/origin/main", &old],
    );
    std::fs::write(fixture.repository.join("local-only"), "new").unwrap();
    commit(&fixture.repository, "local ahead");
    let local = create(&fixture, "local");
    let mut request = create_input(&fixture, "remote");
    request.mode = WorktreeCreateMode::BranchOff {
        base_ref: Some("origin/main".to_owned()),
        branch_name: "remote".to_owned(),
    };
    let remote = fixture.adapter().create(&request).unwrap();
    assert_eq!(
        head(Path::new(&local.worktree_path)),
        head(&fixture.repository)
    );
    assert_eq!(head(Path::new(&remote.worktree_path)), old);
    assert_eq!(
        remote.comparison_base_ref.as_deref(),
        Some("refs/remotes/origin/main")
    );
    let tracking = Command::new("git")
        .args(["config", "--get", "branch.remote.remote"])
        .current_dir(&fixture.repository)
        .output()
        .unwrap();
    assert_eq!(tracking.status.code(), Some(1));
}

#[test]
fn invalid_slug_is_rejected_before_allocating_managed_storage() {
    let fixture = Fixture::new();
    for slug in [
        "",
        "../escape",
        "UPPER",
        "two words",
        "--option",
        "trailing-",
        "a//b",
    ] {
        assert!(
            matches!(
                fixture.adapter().create(&create_input(&fixture, slug)),
                Err(WorktreeError::Invalid(_))
            ),
            "{slug}"
        );
    }
    assert!(!fixture.managed_root.exists());
    assert_eq!(branch(&fixture.repository), "main");
}

#[test]
fn checkout_does_not_treat_a_tag_as_a_branch() {
    let fixture = Fixture::new();
    run(&fixture.repository, &["tag", "release-tag"]);
    let mut request = create_input(&fixture, "tag");
    request.mode = WorktreeCreateMode::Checkout {
        branch_name: "release-tag".to_owned(),
    };
    assert_eq!(
        fixture.adapter().create(&request),
        Err(WorktreeError::UnknownBranch("release-tag".to_owned()))
    );
    assert!(
        fixture
            .adapter()
            .list(&path(&fixture.repository))
            .unwrap()
            .is_empty()
    );
}

#[test]
fn recovery_rejects_a_workspace_directory_outside_its_saved_worktree() {
    let fixture = Fixture::new();
    let mut request = archived(&fixture, "recover");
    request.workspace_cwd = path(&fixture.root.path().join("outside"));
    assert!(matches!(
        fixture.adapter().restore_worktree(&request),
        Err(WorkspaceRecoveryRuntimeError::Invalid(_))
    ));
    assert!(!Path::new(&request.previous_worktree_root).exists());
    assert!(
        fixture
            .adapter()
            .list(&path(&fixture.repository))
            .unwrap()
            .is_empty()
    );
}

#[test]
fn recovery_preserves_an_existing_directory_instead_of_overwriting_it() {
    let fixture = Fixture::new();
    let request = archived(&fixture, "recover");
    std::fs::create_dir(&request.previous_worktree_root).unwrap();
    let marker = Path::new(&request.previous_worktree_root).join("user-file");
    std::fs::write(&marker, "keep").unwrap();
    assert!(matches!(
        fixture.adapter().restore_worktree(&request),
        Err(WorkspaceRecoveryRuntimeError::Invalid(_))
    ));
    assert_eq!(std::fs::read_to_string(marker).unwrap(), "keep");
}

#[test]
fn recovery_reports_a_missing_saved_branch_without_falling_back_to_the_base() {
    let fixture = Fixture::new();
    let request = archived(&fixture, "recover");
    run(&fixture.repository, &["branch", "-D", "recover"]);
    assert_eq!(
        fixture.adapter().restore_worktree(&request),
        Err(WorkspaceRecoveryRuntimeError::UnknownBranch(
            "recover".to_owned()
        ))
    );
    assert!(!Path::new(&request.previous_worktree_root).exists());
    assert_eq!(branch(&fixture.repository), "main");
}

#[test]
fn recovery_cleans_up_a_checkout_missing_the_saved_nested_workspace() {
    let fixture = Fixture::new();
    let mut request = archived(&fixture, "recover");
    request.workspace_cwd =
        path(&Path::new(&request.previous_worktree_root).join("missing/nested"));
    assert!(matches!(
        fixture.adapter().restore_worktree(&request),
        Err(WorkspaceRecoveryRuntimeError::Invalid(_))
    ));
    assert!(!Path::new(&request.previous_worktree_root).exists());
    assert!(
        fixture
            .adapter()
            .list(&path(&fixture.repository))
            .unwrap()
            .is_empty()
    );
    run(
        &fixture.repository,
        &["show-ref", "--verify", "refs/heads/recover"],
    );
}
