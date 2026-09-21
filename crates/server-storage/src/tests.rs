use super::*;
use server_domain::{MessageId, OwnerEpoch, Project, ProjectId, RootMessage};
use server_ports::{Catalog, CatalogEntry, ProjectStorage};

fn project() -> Project {
    Project::new(
        ProjectId::generate(),
        "test".to_owned(),
        "a".repeat(40).parse().unwrap(),
        RootMessage::new(MessageId::generate(), "frozen instructions".to_owned(), 42).unwrap(),
    )
    .unwrap()
}

#[test]
fn project_initialization_reopen_fencing_and_message_immutability() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::create_dir(directory.path().join(".ait-server")).unwrap();
    let original = project();
    let mut first = SqliteProjects.open(directory.path()).unwrap();
    assert_eq!(first.initialize(&original).unwrap(), original);
    assert_eq!(first.initialize(&project()).unwrap(), original);
    assert_eq!(
        first.check_owner(OwnerEpoch::new(0).unwrap()),
        Err(ProjectError::StaleOwner)
    );
    assert_eq!(first.owner_epoch().unwrap().value(), 0);
    let epoch = OwnerEpoch::new(1).unwrap();
    first.claim(epoch).unwrap();
    first.check_owner(epoch).unwrap();
    let mut second = SqliteProjects.open(directory.path()).unwrap();
    assert_eq!(second.initialize(&project()).unwrap(), original);
    second.claim(OwnerEpoch::new(2).unwrap()).unwrap();
    assert_eq!(first.check_owner(epoch), Err(ProjectError::StaleOwner));
    let database = Connection::open(directory.path().join(".ait-server/project.sqlite3")).unwrap();
    for query in [
        "UPDATE messages SET text='changed'",
        "DELETE FROM messages",
        "UPDATE project SET name='changed'",
        "DELETE FROM project",
    ] {
        assert!(database.execute(query, []).is_err());
    }
    assert_eq!(
        database
            .query_row("SELECT count(*) FROM messages", [], |row| row
                .get::<_, u64>(0))
            .unwrap(),
        1
    );
    database
        .execute("UPDATE ownership SET epoch=9223372036854775807", [])
        .unwrap();
    assert_eq!(second.claim(epoch), Err(ProjectError::StaleOwner));
}

#[test]
fn catalog_recovers_intent_and_receipt_across_restart() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("repository");
    let entry = CatalogEntry::from_project(&project(), path.clone());
    let mut catalog = SqliteCatalog::open(directory.path()).unwrap();
    let intent = catalog.begin_open("key", &path).unwrap();
    assert!(intent.receipt.is_none());
    drop(catalog);
    let mut catalog = SqliteCatalog::open(directory.path()).unwrap();
    let restored = catalog.begin_open("key", &path).unwrap();
    assert_eq!(restored.operation_id, intent.operation_id);
    assert!(matches!(
        catalog.begin_open("key", directory.path()),
        Err(ProjectError::IdempotencyConflict)
    ));
    let receipt = catalog.finish_open(&restored, &entry).unwrap();
    assert_eq!(
        catalog.begin_open("key", &path).unwrap().receipt,
        Some(receipt)
    );
    assert_eq!(catalog.finish_open(&restored, &entry).unwrap(), receipt);
    assert_eq!(catalog.get(entry.id).unwrap(), entry);
    assert_eq!(catalog.list(None, 1).unwrap(), std::slice::from_ref(&entry));
    assert!(catalog.list(Some(entry.id), 50).unwrap().is_empty());
    assert_eq!(catalog.list(None, 0), Err(ProjectError::Invalid));
    assert_eq!(
        catalog.get(ProjectId::generate()),
        Err(ProjectError::NotFound)
    );
    let close = catalog.finish_close("key", entry.id).unwrap();
    assert_eq!(catalog.close_receipt("key", entry.id).unwrap(), Some(close));
    assert_eq!(catalog.finish_close("key", entry.id).unwrap(), close);
    assert!(matches!(
        catalog.close_receipt("key", ProjectId::generate()),
        Err(ProjectError::IdempotencyConflict)
    ));
    assert!(catalog.close_receipt("unused", entry.id).unwrap().is_none());
    let mut changed = entry.clone();
    changed.name = "different".to_owned();
    assert_eq!(
        catalog.finish_open(&intent, &changed),
        Err(ProjectError::IdentityConflict)
    );
    changed.path = directory.path().to_owned();
    assert_eq!(
        catalog.finish_open(&intent, &changed),
        Err(ProjectError::IdentityConflict)
    );
    let unregistered = catalog.begin_open("other", &path).unwrap();
    let conflict = CatalogEntry::from_project(&project(), path);
    assert_eq!(
        catalog.finish_open(&unregistered, &conflict),
        Err(ProjectError::IdentityConflict)
    );
    assert!(
        catalog
            .begin_open("other", &conflict.path)
            .unwrap()
            .receipt
            .is_none()
    );
}

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
            Err(ProjectError::UnsupportedFormat)
        ));
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(directory.path().join("catalog.sqlite3"), "not sqlite").unwrap();
    assert!(matches!(
        SqliteCatalog::open(directory.path()),
        Err(ProjectError::UnsupportedFormat)
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
            Err(ProjectError::UnsupportedFormat)
        ));
        std::fs::remove_file(link).unwrap();
    }
    assert_eq!(std::fs::read_to_string(target).unwrap(), "preserve");
}
