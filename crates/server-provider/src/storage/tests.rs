use super::*;

#[test]
fn rejects_foreign_databases_without_rewriting_them() {
    for schema in [
        "CREATE TABLE legacy(data TEXT);",
        "PRAGMA application_id=1234; PRAGMA user_version=1;",
        "PRAGMA user_version=2;",
    ] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("catalog.sqlite3");
        let connection = Connection::open(&path).unwrap();
        connection.execute_batch(schema).unwrap();
        drop(connection);
        let before = std::fs::read(&path).unwrap();
        assert!(matches!(
            SqliteCatalog::open(directory.path()),
            Err(AgentError::UnsupportedFormat)
        ));
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(directory.path().join("catalog.sqlite3"), "not sqlite").unwrap();
    assert!(matches!(
        SqliteCatalog::open(directory.path()),
        Err(AgentError::UnsupportedFormat)
    ));
}

#[cfg(unix)]
#[test]
fn refuses_symlink_databases_and_sidecars() {
    let directory = tempfile::tempdir().unwrap();
    let target = directory.path().join("target");
    std::fs::write(&target, "preserve").unwrap();
    for name in ["catalog.sqlite3", "catalog.sqlite3-wal"] {
        let link = directory.path().join(name);
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert!(matches!(
            SqliteCatalog::open(directory.path()),
            Err(AgentError::UnsupportedFormat)
        ));
        std::fs::remove_file(link).unwrap();
    }
    assert_eq!(std::fs::read_to_string(target).unwrap(), "preserve");
}
