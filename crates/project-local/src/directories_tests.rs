use std::{
    fs,
    sync::{Arc, Barrier},
    thread,
};

use ait_domain::ErrorCode;
use ait_ports::ProjectDirectoryCreator;
use tempfile::TempDir;

use crate::DocumentsProjectDirectory;

fn creator(root: &TempDir) -> DocumentsProjectDirectory {
    let path = root.path().to_path_buf();
    DocumentsProjectDirectory::with_resolver(move || Some(path.clone()))
}

#[test]
fn named_directory_preserves_unicode_and_spaces_and_rejects_existing_entries() {
    let root = TempDir::new().unwrap();
    let creator = creator(&root);
    let path = creator.create_workdir("中文 project").unwrap();
    assert_eq!(
        path,
        root.path().canonicalize().unwrap().join("中文 project")
    );
    fs::write(path.join("keep"), b"untouched").unwrap();
    assert_eq!(
        creator.create_workdir("中文 project").unwrap_err().code,
        ErrorCode::ProjectPathAlreadyExists
    );
    assert_eq!(fs::read(path.join("keep")).unwrap(), b"untouched");
    assert_eq!(fs::read_dir(&path).unwrap().count(), 1);
    fs::write(root.path().join("file"), b"existing file").unwrap();
    assert_eq!(
        creator.create_workdir("file").unwrap_err().code,
        ErrorCode::ProjectPathAlreadyExists
    );
    assert_eq!(
        fs::read(root.path().join("file")).unwrap(),
        b"existing file"
    );
}

#[test]
fn invalid_names_fail_before_resolving_or_writing_documents() {
    let creator =
        DocumentsProjectDirectory::with_resolver(|| panic!("invalid name resolved Documents"));
    for name in [
        "",
        " ",
        ".",
        "..",
        "../escape",
        "a/b",
        "a\\b",
        "/absolute",
        "C:escape",
        "nul",
        "con.txt",
        "LPT1",
        "COM¹.txt",
        "a.",
        " padded",
        "padded ",
        "a\0b",
        "a\nb",
        "a?b",
        &"a".repeat(256),
    ] {
        assert_eq!(
            creator.create_workdir(name).unwrap_err().code,
            ErrorCode::InvalidProject,
            "{name:?}"
        );
    }
}

#[test]
fn unavailable_documents_has_no_fallback_and_does_not_create_parents() {
    let root = TempDir::new().unwrap();
    let missing = root.path().join("missing");
    let file = root.path().join("file");
    fs::write(&file, "keep").unwrap();
    for path in [
        None,
        Some(missing.clone()),
        Some(file.clone()),
        Some("relative".into()),
    ] {
        let creator = DocumentsProjectDirectory::with_resolver(move || path.clone());
        let error = creator.create_workdir("project").unwrap_err();
        assert_eq!(error.code, ErrorCode::ProjectDefaultDirectoryUnavailable);
        assert!(error.message.contains("Documents"));
    }
    assert!(!missing.exists());
    assert_eq!(fs::read_to_string(&file).unwrap(), "keep");
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
}

#[test]
fn concurrent_allocation_has_exactly_one_winner() {
    let root = TempDir::new().unwrap();
    let creator = Arc::new(creator(&root));
    let barrier = Arc::new(Barrier::new(2));
    let workers: Vec<_> = (0..2)
        .map(|_| {
            let creator = creator.clone();
            let barrier = barrier.clone();
            thread::spawn(move || {
                barrier.wait();
                creator.create_workdir("same")
            })
        })
        .collect();
    let results: Vec<_> = workers
        .into_iter()
        .map(|worker| worker.join().unwrap())
        .collect();
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(
        results.into_iter().find_map(Result::err).unwrap().code,
        ErrorCode::ProjectPathAlreadyExists
    );
}

#[cfg(unix)]
#[test]
fn existing_symlinks_including_dangling_links_are_never_followed() {
    use std::os::unix::fs::symlink;
    let root = TempDir::new().unwrap();
    let outside = TempDir::new().unwrap();
    fs::write(outside.path().join("keep"), "untouched").unwrap();
    symlink(outside.path(), root.path().join("link")).unwrap();
    symlink(outside.path().join("absent"), root.path().join("dangling")).unwrap();
    for name in ["link", "dangling"] {
        assert_eq!(
            creator(&root).create_workdir(name).unwrap_err().code,
            ErrorCode::ProjectPathAlreadyExists
        );
        assert!(
            fs::symlink_metadata(root.path().join(name))
                .unwrap()
                .is_symlink()
        );
    }
    assert_eq!(fs::read_dir(outside.path()).unwrap().count(), 1);
    assert_eq!(
        fs::read_to_string(outside.path().join("keep")).unwrap(),
        "untouched"
    );
}

#[cfg(unix)]
#[test]
fn unwritable_documents_reports_creation_failure() {
    use std::os::unix::fs::PermissionsExt;
    let root = TempDir::new().unwrap();
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o500)).unwrap();
    let result = creator(&root).create_workdir("project");
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    // Root can bypass directory permissions; do not claim denial coverage there.
    if result.is_ok() {
        assert_eq!(
            std::process::Command::new("id")
                .arg("-u")
                .output()
                .unwrap()
                .stdout,
            b"0\n"
        );
        return;
    }
    assert_eq!(
        result.unwrap_err().code,
        ErrorCode::ProjectDirectoryCreationFailed
    );
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 0);
}
