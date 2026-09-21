//! Independent project persistence, ownership, crash-window recovery, and real WS RPCs.

#[path = "projects/drain.rs"]
mod drain;
#[path = "projects/fixture.rs"]
mod fixture;
#[path = "projects/transport.rs"]
mod transport;

use std::path::Path;

use fixture::Fixture;
use server_domain::{MessageId, OwnerEpoch, Project, ProjectId, RootMessage};
use server_ports::{Catalog, ProjectError, ProjectStorage, Workspace};
use server_storage::{SqliteCatalog, SqliteProjects};

#[test]
fn open_close_retries_and_restart_preserve_identity_and_history() {
    let fixture = Fixture::new();
    let mut app = fixture.application("state");
    let receipt = app.open(&fixture.repo, "open").unwrap();
    let first = app.get(receipt.project_id).unwrap();
    assert_eq!(app.open(&fixture.repo, "open").unwrap(), receipt);
    assert_eq!(
        app.open(&fixture.repo, "second-open").unwrap().project_id,
        receipt.project_id
    );
    assert_eq!(
        app.get(receipt.project_id).unwrap().owner_epoch,
        first.owner_epoch
    );
    assert_eq!(app.list(None, 50).unwrap().len(), 1);
    assert!(app.list(None, 51).is_err());
    assert!(app.get(ProjectId::generate()).is_err());
    assert_eq!(
        app.close(receipt.project_id, OwnerEpoch::new(0).unwrap(), "close"),
        Err(ProjectError::StaleOwner)
    );
    let close = app
        .close(receipt.project_id, first.owner_epoch.unwrap(), "close")
        .unwrap();
    assert!(app.get(receipt.project_id).unwrap().owner_epoch.is_none());
    assert_eq!(app.open(&fixture.repo, "open").unwrap(), receipt);
    assert!(app.get(receipt.project_id).unwrap().owner_epoch.is_none());
    assert_eq!(
        app.close(receipt.project_id, first.owner_epoch.unwrap(), "new-close"),
        Err(ProjectError::NotOpen)
    );
    let reopen = app.open(&fixture.repo, "reopen").unwrap();
    assert_eq!(reopen.project_id, receipt.project_id);
    assert_eq!(
        app.close(receipt.project_id, first.owner_epoch.unwrap(), "close")
            .unwrap(),
        close
    );
    let reopened = app.get(receipt.project_id).unwrap();
    assert!(reopened.owner_epoch.unwrap().value() > first.owner_epoch.unwrap().value());
    assert_eq!(
        app.close(receipt.project_id, first.owner_epoch.unwrap(), "stale"),
        Err(ProjectError::StaleOwner)
    );
    drop(app);
    std::fs::write(
        fixture.repo.join("AGENTS.md"),
        "changed after initial snapshot",
    )
    .unwrap();
    fixture::git(
        &fixture.repo,
        &["commit", "--allow-empty", "--quiet", "-m", "later HEAD"],
    );
    let mut restarted = fixture.application("state");
    assert!(
        restarted
            .get(receipt.project_id)
            .unwrap()
            .owner_epoch
            .is_none()
    );
    assert_eq!(restarted.open(&fixture.repo, "open").unwrap(), receipt);
    assert!(
        restarted
            .get(receipt.project_id)
            .unwrap()
            .owner_epoch
            .is_none()
    );
    restarted.open(&fixture.repo, "after-restart").unwrap();
    assert_eq!(
        restarted.get(receipt.project_id).unwrap().entry,
        first.entry
    );
    let mut store = SqliteProjects.open(&fixture.repo).unwrap();
    let restored = store.initialize(&initial("unused")).unwrap();
    assert_eq!(restored.root().text(), "original project instructions");
    assert_eq!(restored.root().id(), first.entry.root_message_id);
}

#[test]
fn different_catalogs_and_copied_identities_cannot_own_one_project() {
    let fixture = Fixture::new();
    let mut first = fixture.application("first");
    let mut second = fixture.application("second");
    let receipt = first.open(&fixture.repo, "first").unwrap();
    assert_eq!(
        second.open(&fixture.repo, "second"),
        Err(ProjectError::Busy)
    );
    assert!(second.list(None, 20).unwrap().is_empty());
    let copy = fixture.repository("copy");
    std::fs::create_dir(copy.join(".ait-server")).unwrap();
    std::fs::copy(
        fixture.repo.join(".ait-server/project.sqlite3"),
        copy.join(".ait-server/project.sqlite3"),
    )
    .unwrap();
    assert_eq!(second.open(&copy, "copy"), Err(ProjectError::Busy));
    assert_eq!(
        first.open(&copy, "copy"),
        Err(ProjectError::IdentityConflict)
    );
    drop(first);
    assert_eq!(
        second.open(&copy, "copy").unwrap().project_id,
        receipt.project_id
    );
    assert_eq!(
        second.open(&fixture.repo, "second"),
        Err(ProjectError::IdentityConflict)
    );
}

#[test]
fn switching_between_old_copies_never_reuses_an_owner_generation() {
    let fixture = Fixture::new();
    let mut app = fixture.application("state");
    let receipt = app.open(&fixture.repo, "original").unwrap();
    let original_epoch = app.get(receipt.project_id).unwrap().owner_epoch.unwrap();
    let copy = fixture.repository("copy");
    std::fs::create_dir(copy.join(".ait-server")).unwrap();
    std::fs::copy(
        fixture.repo.join(".ait-server/project.sqlite3"),
        copy.join(".ait-server/project.sqlite3"),
    )
    .unwrap();
    app.close(receipt.project_id, original_epoch, "close-original")
        .unwrap();
    app.open(&copy, "copy").unwrap();
    let copy_epoch = app.get(receipt.project_id).unwrap().owner_epoch.unwrap();
    app.close(receipt.project_id, copy_epoch, "close-copy")
        .unwrap();
    app.open(&fixture.repo, "back-to-original").unwrap();
    let latest = app.get(receipt.project_id).unwrap().owner_epoch.unwrap();
    assert!(latest.value() > copy_epoch.value());
    assert_eq!(
        app.close(receipt.project_id, copy_epoch, "delayed-close"),
        Err(ProjectError::StaleOwner)
    );
}

#[test]
fn project_commit_before_catalog_completion_recovers_and_catalog_can_be_rebuilt() {
    let fixture = Fixture::new();
    let state = fixture.state("state");
    let mut catalog = SqliteCatalog::open(&state).unwrap();
    let workspace = fixture.workspace();
    let info = workspace.inspect(&fixture.repo).unwrap();
    let intent = catalog.begin_open("interrupted", &info.root).unwrap();
    let lease = workspace.acquire_path(&info).unwrap();
    let mut store = SqliteProjects.open(&info.root).unwrap();
    let project = store
        .initialize(&initial("committed before crash"))
        .unwrap();
    // Simulate process death after authoritative Project commit but before catalog receipt.
    drop(store);
    drop(lease);
    drop(catalog);
    let mut recovered = fixture.application("state");
    let receipt = recovered.open(&fixture.repo, "interrupted").unwrap();
    assert_eq!(receipt.operation_id, intent.operation_id);
    assert_eq!(receipt.project_id, project.id());
    assert_eq!(
        recovered.get(project.id()).unwrap().entry.root_message_id,
        project.root().id()
    );
    drop(recovered);
    let mut fresh_catalog = fixture.application("rebuilt-catalog");
    assert_eq!(
        fresh_catalog
            .open(&fixture.repo, "register")
            .unwrap()
            .project_id,
        project.id()
    );
    assert_eq!(
        fresh_catalog
            .get(project.id())
            .unwrap()
            .entry
            .root_message_id,
        project.root().id()
    );
}

#[test]
fn invalid_inputs_do_not_register_or_modify_legacy_projects() {
    let fixture = Fixture::new();
    let mut app = fixture.application("state");
    assert_eq!(
        app.open(&fixture.repo, "bad key"),
        Err(ProjectError::Invalid)
    );
    std::fs::create_dir(fixture.repo.join(".ait")).unwrap();
    let legacy = fixture.repo.join(".ait/project.sqlite3");
    std::fs::write(&legacy, "legacy bytes").unwrap();
    assert_eq!(
        app.open(&fixture.repo, "legacy"),
        Err(ProjectError::LegacyProject)
    );
    assert!(!fixture.repo.join(".ait-server").exists());
    assert_eq!(std::fs::read_to_string(legacy).unwrap(), "legacy bytes");
    assert!(app.list(None, 20).unwrap().is_empty());
    assert!(
        app.open(Path::new("/missing-server-test-project"), "missing")
            .is_err()
    );
}

#[cfg(unix)]
#[test]
fn canonical_aliases_dedupe_and_changed_parameters_conflict() {
    let fixture = Fixture::new();
    let alias = fixture.directory.path().join("alias");
    std::os::unix::fs::symlink(&fixture.repo, &alias).unwrap();
    let mut app = fixture.application("state");
    let receipt = app.open(&alias, "open").unwrap();
    assert_eq!(app.open(&fixture.repo, "open").unwrap(), receipt);
    assert_eq!(
        app.open(&fixture.repository("other"), "open"),
        Err(ProjectError::IdempotencyConflict)
    );
    assert_eq!(app.list(None, 20).unwrap().len(), 1);
}

fn initial(text: &str) -> Project {
    Project::new(
        ProjectId::generate(),
        "initial".to_owned(),
        "b".repeat(40).parse().unwrap(),
        RootMessage::new(MessageId::generate(), text.to_owned(), 42).unwrap(),
    )
    .unwrap()
}
