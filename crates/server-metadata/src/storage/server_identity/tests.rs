use super::*;

#[test]
fn identity_survives_reopen_and_invalid_state_is_preserved() {
    let directory = tempfile::tempdir().unwrap();
    let id = load_or_create(directory.path()).unwrap();
    assert!(!id.is_nil());
    assert_eq!(load_or_create(directory.path()).unwrap(), id);
    let path = directory.path().join("server-id");
    for invalid in ["broken", "00000000-0000-0000-0000-000000000000"] {
        fs::write(&path, invalid).unwrap();
        assert!(matches!(
            load_or_create(directory.path()),
            Err(IdentityError::Invalid)
        ));
        assert_eq!(fs::read_to_string(&path).unwrap(), invalid);
    }
    assert!(matches!(
        load_or_create(&directory.path().join("missing")),
        Err(IdentityError::Io(_))
    ));
}

#[cfg(unix)]
#[test]
fn identity_rejects_symlinks_without_changing_the_target() {
    let directory = tempfile::tempdir().unwrap();
    let target = directory.path().join("target");
    fs::write(&target, Uuid::new_v4().to_string()).unwrap();
    std::os::unix::fs::symlink(&target, directory.path().join("server-id")).unwrap();
    let before = fs::read(&target).unwrap();
    assert!(matches!(
        load_or_create(directory.path()),
        Err(IdentityError::Invalid)
    ));
    assert_eq!(fs::read(&target).unwrap(), before);
}
