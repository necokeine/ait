//! Paseo workspace-labels commit, recovery, and concurrent-field invariants.

use std::sync::{Arc, Mutex};

use super::*;
use crate::ports::registry::RegistryError;

fn fixture() -> (
    tempfile::TempDir,
    FileBackedWorkspaceRegistry,
    FileWorkspaceLabelStore,
) {
    let root = tempfile::tempdir().unwrap();
    let registry = FileBackedWorkspaceRegistry::new(root.path().join("projects/workspaces.json"));
    registry
        .upsert(
            &workspace(None, "before"),
            WorkspaceMutationContext::default(),
        )
        .unwrap();
    let store = FileWorkspaceLabelStore::new(root.path(), registry.clone());
    store.initialize().unwrap();
    (root, registry, store)
}

fn mutation() -> WorkspaceLabelStoreMutation {
    WorkspaceLabelStoreMutation {
        require_active_workspace: None,
        expected_labels: Vec::new(),
        labels: vec![urgent()],
        workspace_updates: vec![workspace(Some(vec!["Urgent".to_owned()]), "after")],
    }
}

fn transaction(phase: TransactionPhase) -> Transaction {
    Transaction {
        phase,
        before_labels: Vec::new(),
        after_labels: vec![urgent()],
        before_workspaces: vec![WorkspaceState {
            workspace_id: "wks_one".to_owned(),
            labels: None,
            updated_at: "before".to_owned(),
        }],
        after_workspaces: vec![WorkspaceState {
            workspace_id: "wks_one".to_owned(),
            labels: Some(vec!["Urgent".to_owned()]),
            updated_at: "after".to_owned(),
        }],
    }
}

#[test]
fn catalog_rewrite_preserves_fields_changed_after_the_caller_snapshot() {
    let (_root, registry, store) = fixture();
    let requested = mutation();
    registry
        .update("wks_one", &|record| {
            let mut next = record.clone();
            next.title = Some("Concurrent rename".to_owned());
            next.pinned_at = Some("pinned".to_owned());
            next.archived_at = Some("archived".to_owned());
            next
        })
        .unwrap();
    let events = Arc::new(Mutex::new(Vec::new()));
    let captured = events.clone();
    let _subscription = registry.subscribe_to_mutations(Arc::new(move |event| {
        captured.lock().unwrap().push(event.clone());
        Ok(())
    }));
    store.commit(&requested).unwrap();
    let saved = registry.get("wks_one").unwrap().unwrap();
    assert_eq!(saved.title.as_deref(), Some("Concurrent rename"));
    assert_eq!(saved.pinned_at.as_deref(), Some("pinned"));
    assert_eq!(saved.archived_at.as_deref(), Some("archived"));
    assert_eq!(saved.labels, Some(vec!["Urgent".to_owned()]));
    assert_eq!(events.lock().unwrap()[0].workspace.as_ref(), Some(&saved));
}

#[test]
fn prepared_recovery_rolls_back_labels_without_overwriting_unrelated_fields() {
    let (root, registry, store) = fixture();
    store.commit(&mutation()).unwrap();
    registry
        .update("wks_one", &|record| {
            let mut next = record.clone();
            next.title = Some("Preserved".to_owned());
            next
        })
        .unwrap();
    let journal = root
        .path()
        .join("projects/workspace-labels.transaction.json");
    write_json_atomic(&journal, &transaction(TransactionPhase::Prepared)).unwrap();
    let events = Arc::new(Mutex::new(Vec::new()));
    let captured = events.clone();
    let _subscription = registry.subscribe_to_mutations(Arc::new(move |event| {
        captured.lock().unwrap().push(event.clone());
        Ok(())
    }));
    let recovered = FileWorkspaceLabelStore::new(root.path(), registry.clone());
    let snapshot = recovered.snapshot().unwrap();
    assert!(snapshot.labels.is_empty());
    assert_eq!(snapshot.workspaces[0].labels, None);
    assert_eq!(snapshot.workspaces[0].title.as_deref(), Some("Preserved"));
    assert_eq!(snapshot.workspaces[0].updated_at, "before");
    assert!(events.lock().unwrap().is_empty());
    assert!(!journal.exists());
    assert_eq!(
        FileWorkspaceLabelStore::new(root.path(), registry)
            .snapshot()
            .unwrap(),
        snapshot
    );
}

#[test]
fn committed_recovery_keeps_committed_assignments_and_only_cleans_journal() {
    let (root, registry, store) = fixture();
    store.commit(&mutation()).unwrap();
    let journal = root
        .path()
        .join("projects/workspace-labels.transaction.json");
    write_json_atomic(&journal, &transaction(TransactionPhase::Committed)).unwrap();
    let recovered = FileWorkspaceLabelStore::new(root.path(), registry);
    let snapshot = recovered.snapshot().unwrap();
    assert_eq!(snapshot.labels, vec![urgent()]);
    assert_eq!(
        snapshot.workspaces[0].labels,
        Some(vec!["Urgent".to_owned()])
    );
    assert_eq!(snapshot.workspaces[0].updated_at, "after");
    assert!(!journal.exists());
}

#[test]
fn unknown_workspace_aborts_before_writing_catalog_or_journal() {
    let (root, registry, store) = fixture();
    let before = fs::read(root.path().join("projects/workspaces.json")).unwrap();
    let mut requested = mutation();
    requested.workspace_updates[0].workspace_id = "missing".to_owned();
    assert_eq!(
        store.commit(&requested),
        Err(WorkspaceLabelStoreError::Invalid)
    );
    assert_eq!(registry.get("wks_one").unwrap().unwrap().labels, None);
    assert!(store.snapshot().unwrap().labels.is_empty());
    assert_eq!(
        fs::read(root.path().join("projects/workspaces.json")).unwrap(),
        before
    );
    assert!(
        !root
            .path()
            .join("projects/workspace-labels.transaction.json")
            .exists()
    );
}

#[test]
fn duplicate_catalog_identity_aborts_assignment_and_durable_write() {
    let (root, registry, store) = fixture();
    let mut requested = mutation();
    requested.labels.push(WorkspaceLabelDefinition {
        name: "urgent".to_owned(),
        color: WorkspaceLabelColor::Sky,
    });
    assert_eq!(
        store.commit(&requested),
        Err(WorkspaceLabelStoreError::Invalid)
    );
    assert!(store.snapshot().unwrap().labels.is_empty());
    assert_eq!(registry.get("wks_one").unwrap().unwrap().labels, None);
    assert!(
        !root
            .path()
            .join("projects/workspace-labels.transaction.json")
            .exists()
    );
}

#[test]
fn workspace_listener_failure_does_not_roll_back_a_durable_label_commit() {
    let (root, registry, store) = fixture();
    let _subscription = registry.subscribe_to_mutations(Arc::new(|_| Err(RegistryError::Observer)));
    store.commit(&mutation()).unwrap();
    let reopened = FileWorkspaceLabelStore::new(
        root.path(),
        FileBackedWorkspaceRegistry::new(root.path().join("projects/workspaces.json")),
    );
    let snapshot = reopened.snapshot().unwrap();
    assert_eq!(snapshot.labels, vec![urgent()]);
    assert_eq!(
        snapshot.workspaces[0].labels,
        Some(vec!["Urgent".to_owned()])
    );
}

#[test]
fn no_op_commit_does_not_create_catalog_transaction_or_mutation_events() {
    let (root, registry, store) = fixture();
    let events = Arc::new(Mutex::new(Vec::new()));
    let captured = events.clone();
    let _subscription = registry.subscribe_to_mutations(Arc::new(move |event| {
        captured.lock().unwrap().push(event.clone());
        Ok(())
    }));
    store
        .commit(&WorkspaceLabelStoreMutation {
            require_active_workspace: None,
            expected_labels: Vec::new(),
            labels: Vec::new(),
            workspace_updates: Vec::new(),
        })
        .unwrap();
    assert!(events.lock().unwrap().is_empty());
    assert!(!root.path().join("projects/workspace-labels.json").exists());
    assert!(
        !root
            .path()
            .join("projects/workspace-labels.transaction.json")
            .exists()
    );
}

#[test]
fn empty_normalized_catalog_name_aborts_before_persistence() {
    let (root, registry, store) = fixture();
    let mut requested = mutation();
    requested.labels[0].name = " \t\n ".to_owned();
    assert_eq!(
        store.commit(&requested),
        Err(WorkspaceLabelStoreError::Invalid)
    );
    assert!(store.snapshot().unwrap().labels.is_empty());
    assert_eq!(registry.get("wks_one").unwrap().unwrap().labels, None);
    assert!(!root.path().join("projects/workspace-labels.json").exists());
}

#[test]
fn malformed_duplicate_catalog_is_rejected_after_restart_without_rewriting_it() {
    let (root, registry, _) = fixture();
    let path = root.path().join("projects/workspace-labels.json");
    let duplicate = vec![
        urgent(),
        WorkspaceLabelDefinition {
            name: "  urgent  ".to_owned(),
            color: WorkspaceLabelColor::Sky,
        },
    ];
    write_json_atomic(&path, &duplicate).unwrap();
    let bytes = fs::read(&path).unwrap();
    let reopened = FileWorkspaceLabelStore::new(root.path(), registry);
    assert_eq!(
        reopened.initialize(),
        Err(WorkspaceLabelStoreError::Invalid)
    );
    assert_eq!(fs::read(path).unwrap(), bytes);
}
