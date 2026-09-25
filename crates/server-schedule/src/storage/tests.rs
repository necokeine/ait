use super::*;
#[test]
fn missing_atomic_roundtrip_and_invalid_version() {
    let root = tempfile::tempdir().unwrap();
    let path = root
        .path()
        .canonicalize()
        .unwrap()
        .join("schedules/data.json");
    let mut store = FileStore::new(path.clone());
    assert!(store.load().unwrap().is_empty());
    store.save(&[]).unwrap();
    assert!(store.load().unwrap().is_empty());
    fs::write(&path, r#"{"version":2,"schedules":[]}"#).unwrap();
    assert_eq!(store.load().unwrap_err(), Error::Storage);
    fs::write(&path, "broken").unwrap();
    assert_eq!(store.load().unwrap_err(), Error::Storage);
}
#[cfg(unix)]
#[test]
fn refuses_symlink_targets_and_keeps_private_mode() {
    use std::os::unix::{fs::PermissionsExt, fs::symlink};
    let root = tempfile::tempdir().unwrap();
    let path = root.path().canonicalize().unwrap().join("data.json");
    let mut store = FileStore::new(path.clone());
    store.save(&[]).unwrap();
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let link = path.with_extension("link");
    symlink(&path, &link).unwrap();
    let mut bad = FileStore::new(link);
    assert!(bad.load().is_err());
    assert!(bad.save(&[]).is_err());
    assert!(store.load().unwrap().is_empty());
}
#[test]
fn rejects_parent_traversal_and_failed_replace() {
    let root = tempfile::tempdir().unwrap();
    let mut store = FileStore::new(root.path().join("../data"));
    assert!(store.save(&[]).is_err());
    let mut store = FileStore::new(root.path().canonicalize().unwrap());
    assert!(store.save(&[]).is_err());
}
