use super::{LocalProjectEnvironment, ProjectPathGuard};
use ait_ports::ProjectEnvironment;
use std::{fs, path::Path};
use tempfile::TempDir;

#[cfg(unix)]
#[test]
fn symlink_escape_is_rejected_for_reads_and_creates() {
    use std::os::unix::fs::symlink;

    let project = TempDir::new().unwrap();
    let outside = TempDir::new().unwrap();
    fs::write(outside.path().join("secret.md"), "secret").unwrap();
    symlink(outside.path(), project.path().join("escape")).unwrap();
    let environment = LocalProjectEnvironment;
    let guard = ProjectPathGuard::new(project.path()).unwrap();

    let read = environment.read_project_file(project.path(), Path::new("escape/secret.md"));
    assert!(matches!(
        read,
        Err(ait_ports::EnvironmentError::OutOfScope(_))
    ));
    let create = guard.resolve_for_creation(Path::new("escape/new.txt"));
    assert!(matches!(
        create,
        Err(ait_ports::EnvironmentError::OutOfScope(_))
    ));
}

#[test]
fn missing_external_file_outside_the_authorized_root_is_still_rejected() {
    let authorized = TempDir::new().unwrap();
    let outside = TempDir::new().unwrap();
    let missing = outside.path().join("missing.md");
    let environment = LocalProjectEnvironment;

    let result = environment.read_authorized_file(authorized.path(), &missing);

    assert!(matches!(
        result,
        Err(ait_ports::EnvironmentError::OutOfScope(_))
    ));
}
