use super::*;
use server_domain::agent::Driver;

fn create(key: &str) -> ConfigureAgent {
    ConfigureAgent {
        target: AgentTarget::Create,
        config: config("original", true),
        key: key.to_owned(),
        recorded_at: 42,
    }
}

fn config(name: &str, enabled: bool) -> AgentConfig {
    AgentConfig::new(
        name.to_owned(),
        Driver::Codex,
        "test-model".to_owned(),
        Some("env:AIT_SERVER_CREDENTIAL_TEST".parse().unwrap()),
        enabled,
    )
    .unwrap()
}

#[test]
fn immutable_heads_receipts_pages_and_restart() {
    let directory = tempfile::tempdir().unwrap();
    let mut catalog = SqliteCatalog::open(directory.path()).unwrap();
    let mut command = create("create");
    let original = catalog.configure(&command).unwrap();
    command.recorded_at = 99;
    assert_eq!(catalog.configure(&command).unwrap(), original);
    command.config = config("changed", true);
    assert_eq!(
        catalog.configure(&command),
        Err(AgentError::IdempotencyConflict)
    );
    command.target = AgentTarget::Update {
        id: original.agent_id,
        expected: original.revision,
    };
    command.key = "update".to_owned();
    let updated = catalog.configure(&command).unwrap();
    assert_eq!(updated.revision.value(), 2);
    assert_eq!(catalog.configure(&create("create")).unwrap(), original);
    assert_eq!(
        catalog
            .get_agent(original.agent_id, None)
            .unwrap()
            .config()
            .name(),
        "changed"
    );
    assert_eq!(
        catalog
            .get_agent(original.agent_id, Some(original.revision))
            .unwrap()
            .config()
            .name(),
        "original"
    );
    assert_eq!(
        catalog.get_agent(original.agent_id, Some(Revision::new(3).unwrap())),
        Err(AgentError::RevisionNotFound)
    );
    assert_eq!(
        catalog.get_agent(AgentId::generate(), None),
        Err(AgentError::NotFound)
    );
    command.key = "stale".to_owned();
    assert_eq!(
        catalog.configure(&command),
        Err(AgentError::RevisionConflict)
    );
    let second = catalog.configure(&create("second")).unwrap();
    let first = catalog.list_agents(None, 1).unwrap();
    let rest = catalog.list_agents(Some(first[0].id()), 50).unwrap();
    let mut ids = vec![first[0].id(), rest[0].id()];
    let mut expected = vec![original.agent_id, second.agent_id];
    ids.sort();
    expected.sort();
    assert_eq!(ids, expected);
    assert_eq!(catalog.list_agents(None, 51), Err(AgentError::Invalid));
    for query in [
        "UPDATE agent_revisions SET model='other'",
        "DELETE FROM agent_revisions",
    ] {
        assert!(catalog.0.execute(query, []).is_err());
    }
    drop(catalog);
    let mut catalog = SqliteCatalog::open(directory.path()).unwrap();
    command.key = "update".to_owned();
    assert_eq!(catalog.configure(&command).unwrap(), updated);
    assert_eq!(
        catalog
            .get_agent(original.agent_id, None)
            .unwrap()
            .revision(),
        updated.revision
    );
}

#[test]
fn defaults_are_explicit_versioned_and_replays_do_not_reselect() {
    let directory = tempfile::tempdir().unwrap();
    let mut catalog = SqliteCatalog::open(directory.path()).unwrap();
    let agent = catalog.configure(&create("create")).unwrap();
    assert_eq!(
        catalog.get_default().unwrap(),
        DefaultSelection {
            agent_id: None,
            version: 0
        }
    );
    let select = SelectDefault {
        agent_id: Some(agent.agent_id),
        expected_version: 0,
        key: "select".to_owned(),
    };
    let selected = catalog.set_default(&select).unwrap();
    let disable = ConfigureAgent {
        target: AgentTarget::Update {
            id: agent.agent_id,
            expected: agent.revision,
        },
        config: config("disabled", false),
        key: "disable".to_owned(),
        recorded_at: 43,
    };
    assert_eq!(catalog.configure(&disable), Err(AgentError::IsDefault));
    let mut clear = SelectDefault {
        agent_id: None,
        expected_version: 0,
        key: "clear".to_owned(),
    };
    assert_eq!(
        catalog.set_default(&clear),
        Err(AgentError::DefaultConflict)
    );
    clear.expected_version = 1;
    let cleared = catalog.set_default(&clear).unwrap();
    assert_eq!(catalog.set_default(&select).unwrap(), selected);
    assert_eq!(catalog.get_default().unwrap(), cleared.selection);
    catalog.configure(&disable).unwrap();
    let mut next = SelectDefault {
        agent_id: Some(agent.agent_id),
        expected_version: 2,
        key: "next".to_owned(),
    };
    assert_eq!(catalog.set_default(&next), Err(AgentError::Disabled));
    next.agent_id = Some(AgentId::generate());
    assert_eq!(catalog.set_default(&next), Err(AgentError::NotFound));
    next.key = "select".to_owned();
    assert_eq!(
        catalog.set_default(&next),
        Err(AgentError::IdempotencyConflict)
    );
    drop(catalog);
    let mut catalog = SqliteCatalog::open(directory.path()).unwrap();
    assert_eq!(catalog.set_default(&select).unwrap(), selected);
    assert_eq!(catalog.get_default().unwrap(), cleared.selection);
    catalog
        .0
        .execute("UPDATE agent_default SET version=9223372036854775807", [])
        .unwrap();
    clear.key = "overflow".to_owned();
    clear.expected_version = i64::MAX as u64;
    assert_eq!(catalog.set_default(&clear), Err(AgentError::Invalid));
}

#[test]
fn invalid_domain_snapshot_does_not_publish_configuration_or_receipt() {
    let directory = tempfile::tempdir().unwrap();
    let mut catalog = SqliteCatalog::open(directory.path()).unwrap();
    let mut command = create("clock-boundary");
    command.recorded_at = u64::MAX;
    assert_eq!(catalog.configure(&command), Err(AgentError::Invalid));
    assert!(catalog.list_agents(None, 50).unwrap().is_empty());
    command.recorded_at = 42;
    assert_eq!(catalog.configure(&command).unwrap().revision.value(), 1);
}

#[test]
fn receipt_failure_rolls_back_heads_revisions_and_default() {
    let directory = tempfile::tempdir().unwrap();
    let mut catalog = SqliteCatalog::open(directory.path()).unwrap();
    catalog.0.execute_batch("CREATE TRIGGER fail_receipt BEFORE INSERT ON agent_configure_receipts BEGIN SELECT RAISE(ABORT,'injected'); END;").unwrap();
    assert_eq!(
        catalog.configure(&create("create")),
        Err(AgentError::UnsupportedFormat)
    );
    assert!(catalog.list_agents(None, 50).unwrap().is_empty());
    catalog
        .0
        .execute_batch("DROP TRIGGER fail_receipt;")
        .unwrap();
    let agent = catalog.configure(&create("create")).unwrap();
    catalog.0.execute_batch("CREATE TRIGGER fail_update_receipt BEFORE INSERT ON agent_configure_receipts BEGIN SELECT RAISE(ABORT,'injected'); END;").unwrap();
    let mut update = create("update");
    update.target = AgentTarget::Update {
        id: agent.agent_id,
        expected: agent.revision,
    };
    assert_eq!(
        catalog.configure(&update),
        Err(AgentError::UnsupportedFormat)
    );
    assert_eq!(
        catalog.get_agent(agent.agent_id, None).unwrap().revision(),
        agent.revision
    );
    assert_eq!(
        catalog.get_agent(agent.agent_id, Some(Revision::new(2).unwrap())),
        Err(AgentError::RevisionNotFound)
    );
    catalog
        .0
        .execute_batch("DROP TRIGGER fail_update_receipt;")
        .unwrap();
    catalog.0.execute_batch("CREATE TRIGGER fail_default BEFORE INSERT ON agent_default_receipts BEGIN SELECT RAISE(ABORT,'injected'); END;").unwrap();
    let select = SelectDefault {
        agent_id: Some(agent.agent_id),
        expected_version: 0,
        key: "select".to_owned(),
    };
    assert_eq!(
        catalog.set_default(&select),
        Err(AgentError::UnsupportedFormat)
    );
    assert_eq!(catalog.get_default().unwrap().version, 0);
    catalog
        .0
        .execute_batch("DROP TRIGGER fail_default;")
        .unwrap();
    assert_eq!(catalog.set_default(&select).unwrap().selection.version, 1);
}

#[test]
fn concurrent_editors_publish_one_revision() {
    let directory = tempfile::tempdir().unwrap();
    let mut first = SqliteCatalog::open(directory.path()).unwrap();
    let original = first.configure(&create("create")).unwrap();
    let mut second = SqliteCatalog::open(directory.path()).unwrap();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let other = barrier.clone();
    let mut command = create("one");
    command.target = AgentTarget::Update {
        id: original.agent_id,
        expected: original.revision,
    };
    let mut competitor = command.clone();
    competitor.key = "two".to_owned();
    let task = std::thread::spawn(move || {
        other.wait();
        second.configure(&competitor)
    });
    barrier.wait();
    let results = [first.configure(&command), task.join().unwrap()];
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert!(results.contains(&Err(AgentError::RevisionConflict)));
    assert_eq!(
        first
            .get_agent(original.agent_id, None)
            .unwrap()
            .revision()
            .value(),
        2
    );
}

#[test]
fn v1_upgrade_preserves_retired_tables_and_retains_reopenable_backup() {
    let directory = tempfile::tempdir().unwrap();
    let original = Connection::open(directory.path().join("catalog.sqlite3")).unwrap();
    original.execute_batch("CREATE TABLE operations(key TEXT, id TEXT); INSERT INTO operations VALUES('open','preserved-receipt'); PRAGMA application_id=1095979843; PRAGMA user_version=1;").unwrap();
    drop(original);
    let mut migrated = SqliteCatalog::open(directory.path()).unwrap();
    assert_eq!(
        migrated
            .0
            .query_row("SELECT id FROM operations WHERE key='open'", [], |row| {
                row.get::<_, String>(0)
            })
            .unwrap(),
        "preserved-receipt"
    );
    assert_eq!(migrated.get_default().unwrap().version, 0);
    let receipt = migrated.configure(&create("create")).unwrap();
    drop(migrated);
    let backups: Vec<_> = std::fs::read_dir(directory.path())
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("catalog-v1-backup-")
        })
        .collect();
    assert_eq!(backups.len(), 1);
    let backup = Connection::open(&backups[0]).unwrap();
    assert_eq!(
        backup
            .pragma_query_value(None, "user_version", |row| row.get::<_, i32>(0))
            .unwrap(),
        1
    );
    assert_eq!(
        backup
            .query_row("SELECT id FROM operations WHERE key='open'", [], |row| {
                row.get::<_, String>(0)
            })
            .unwrap(),
        "preserved-receipt"
    );
    assert_eq!(
        backup
            .query_row("PRAGMA integrity_check", [], |row| row.get::<_, String>(0))
            .unwrap(),
        "ok"
    );
    let mut reopened = SqliteCatalog::open(directory.path()).unwrap();
    assert_eq!(reopened.configure(&create("create")).unwrap(), receipt);
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 2);
}

#[test]
fn fresh_catalog_only_initializes_agent_tables() {
    let directory = tempfile::tempdir().unwrap();
    let catalog = SqliteCatalog::open(directory.path()).unwrap();
    let retired: usize = catalog
        .0
        .query_row(
            "SELECT count(*) FROM sqlite_schema WHERE name IN ('projects','operations')",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(retired, 0);
}

#[test]
fn migration_failure_rolls_back_version_and_foreign_formats_are_untouched() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("catalog.sqlite3");
    let old = Connection::open(&path).unwrap();
    old.execute_batch("PRAGMA application_id=1095979843; PRAGMA user_version=1; CREATE TABLE agent_default(x TEXT);").unwrap();
    assert!(SqliteCatalog::open(directory.path()).is_err());
    assert_eq!(
        old.pragma_query_value(None, "user_version", |row| row.get::<_, i32>(0))
            .unwrap(),
        1
    );
    assert_eq!(
        old.query_row(
            "SELECT count(*) FROM sqlite_schema WHERE name='agents'",
            [],
            |row| row.get::<_, usize>(0)
        )
        .unwrap(),
        0
    );
    old.execute_batch("PRAGMA user_version=3;").unwrap();
    let before = std::fs::read(&path).unwrap();
    assert!(matches!(
        SqliteCatalog::open(directory.path()),
        Err(AgentError::UnsupportedFormat)
    ));
    assert_eq!(std::fs::read(&path).unwrap(), before);
}
