use super::*;
use std::process::Command;

fn git(root: &Path, args: &[&str]) {
    let result = Command::new("git")
        .args([
            "-c",
            "user.name=Server Test",
            "-c",
            "user.email=server@example.invalid",
        ])
        .args(args)
        .current_dir(root)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
}

fn fixture() -> (tempfile::TempDir, PathBuf, LocalWorkspace) {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("repo");
    std::fs::create_dir(&root).unwrap();
    git(&root, &["init", "--quiet"]);
    git(
        &root,
        &["commit", "--quiet", "--allow-empty", "-m", "baseline"],
    );
    let workspace = LocalWorkspace::new(directory.path().join("identity-locks"));
    (directory, root, workspace)
}

#[test]
fn freezes_instructions_and_excludes_runtime_with_exclusive_leases() {
    let (_directory, root, workspace) = fixture();
    std::fs::write(root.join("AGENTS.md"), "initial instructions").unwrap();
    let info = workspace.inspect(&root).unwrap();
    assert_eq!(info.instructions, "initial instructions");
    assert_eq!(info.root, root.canonicalize().unwrap());
    let lease = workspace.acquire_path(&info).unwrap();
    assert!(matches!(
        workspace.acquire_path(&info),
        Err(ProjectError::Busy)
    ));
    assert_eq!(
        git::run(&root, &["check-ignore", "--", ".ait-server/project.lock"]).unwrap(),
        ".ait-server/project.lock"
    );
    let id = ProjectId::generate();
    let identity = workspace.acquire_identity(id).unwrap();
    assert!(matches!(
        workspace.acquire_identity(id),
        Err(ProjectError::Busy)
    ));
    drop(identity);
    workspace.acquire_identity(id).unwrap();
    drop(lease);
    workspace.acquire_path(&info).unwrap();
    let exclude = std::fs::read_to_string(root.join(".git/info/exclude")).unwrap();
    assert_eq!(
        exclude
            .lines()
            .filter(|line| *line == "/.ait-server/")
            .count(),
        1
    );
    std::fs::write(root.join("AGENTS.md"), "changed").unwrap();
    assert!(matches!(
        workspace.acquire_path(&info),
        Err(ProjectError::UnsupportedWorkspace)
    ));
    std::fs::write(
        root.join("AGENTS.md"),
        "a".repeat(MAX_INSTRUCTION_BYTES + 1),
    )
    .unwrap();
    assert!(matches!(
        workspace.inspect(&root),
        Err(ProjectError::Invalid)
    ));
}

#[test]
fn rejects_legacy_subdirectories_unborn_repositories_and_shared_worktrees() {
    let (directory, root, workspace) = fixture();
    assert!(
        workspace
            .inspect(&directory.path().join("missing"))
            .is_err()
    );
    let subdir = root.join("subdir");
    std::fs::create_dir(&subdir).unwrap();
    assert!(workspace.inspect(&subdir).is_err());
    std::fs::create_dir(root.join(".ait")).unwrap();
    assert!(matches!(
        workspace.inspect(&root),
        Err(ProjectError::LegacyProject)
    ));
    std::fs::remove_dir(root.join(".ait")).unwrap();
    let linked = directory.path().join("linked");
    git(
        &root,
        &["worktree", "add", "--detach", linked.to_str().unwrap()],
    );
    assert!(workspace.inspect(&linked).is_err());
    assert!(matches!(
        workspace.inspect(&root),
        Err(ProjectError::UnsupportedWorkspace)
    ));
    let unborn = directory.path().join("unborn");
    std::fs::create_dir(&unborn).unwrap();
    git(&unborn, &["init", "--quiet"]);
    assert!(matches!(
        workspace.inspect(&unborn),
        Err(ProjectError::UnsupportedWorkspace)
    ));
    assert!(!unborn.join(".ait-server").exists());
}

#[test]
fn refuses_tracked_runtime_or_repository_override_of_local_exclusion() {
    let (_directory, root, workspace) = fixture();
    std::fs::write(root.join(".gitignore"), "!.ait-server/\n").unwrap();
    let info = workspace.inspect(&root).unwrap();
    assert!(matches!(
        workspace.acquire_path(&info),
        Err(ProjectError::UnsupportedWorkspace)
    ));
    assert!(root.join(".ait-server/project.lock").exists());
    std::fs::write(root.join(".ait-server/marker"), "tracked").unwrap();
    git(&root, &["add", "--force", ".ait-server/marker"]);
    assert!(matches!(
        workspace.inspect(&root),
        Err(ProjectError::UnsupportedWorkspace)
    ));
    let full_exclude = "#".repeat(64 * 1024);
    std::fs::write(root.join(".git/info/exclude"), &full_exclude).unwrap();
    assert_eq!(exclude_runtime(&root), Err(ProjectError::Invalid));
    assert_eq!(
        std::fs::read_to_string(root.join(".git/info/exclude")).unwrap(),
        full_exclude
    );
}

#[cfg(unix)]
#[test]
fn aliases_share_path_ownership_but_managed_symlinks_are_rejected() {
    let (directory, root, workspace) = fixture();
    let alias = directory.path().join("alias");
    std::os::unix::fs::symlink(&root, &alias).unwrap();
    let info = workspace.inspect(&alias).unwrap();
    let lease = workspace.acquire_path(&info).unwrap();
    assert!(matches!(
        workspace.acquire_path(&workspace.inspect(&root).unwrap()),
        Err(ProjectError::Busy)
    ));
    drop(lease);
    std::fs::remove_file(root.join(".ait-server/project.lock")).unwrap();
    let external = directory.path().join("external");
    std::fs::write(&external, "preserve").unwrap();
    std::os::unix::fs::symlink(&external, root.join(".ait-server/project.lock")).unwrap();
    assert!(workspace.acquire_path(&info).is_err());
    std::os::unix::fs::symlink(&external, root.join("AGENTS.md")).unwrap();
    assert!(workspace.inspect(&root).is_err());
    assert_eq!(std::fs::read_to_string(external).unwrap(), "preserve");
}
