use std::sync::atomic::{AtomicBool, Ordering};

use super::*;
use crate::model::workspace_labels::{WorkspaceLabelColor, WorkspaceLabelDefinition};
use crate::ports::registry::{RegistryError, WorkspaceMutationContext};
use crate::ports::workspace_labels::{
    WorkspaceLabelStore, WorkspaceLabelStoreError, WorkspaceLabelStoreMutation,
};
use crate::storage::workspace_labels::FileWorkspaceLabelStore;

#[test]
fn label_journal_callback_observes_the_locked_before_image() {
    let root = tempfile::tempdir().unwrap();
    let registry = FileBackedWorkspaceRegistry::new(root.path().join("workspaces.json"));
    let mut current = workspace("one");
    current.updated_at = "latest-update".to_owned();
    registry
        .upsert(&current, WorkspaceMutationContext::default())
        .unwrap();
    let mut after = current.clone();
    after.updated_at = "assignment".to_owned();
    after.labels = Some(vec!["Urgent".to_owned()]);
    registry
        .commit_workspace_label_mutation(
            &[after.clone()],
            |before| {
                assert_eq!(before, &[current]);
                Ok(())
            },
            || Ok(()),
            false,
        )
        .unwrap();
    assert_eq!(registry.get("one").unwrap(), Some(after));
}

#[test]
fn failed_label_commit_restores_the_latest_registry_timestamp_and_stays_silent() {
    // The injected writer fails an actual prepared commit once. The separate callback
    // test verifies where its before-image is captured; no scheduling race is assumed.
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("projects/workspaces.json");
    let mut registry = FileBackedWorkspaceRegistry::new(path.clone());
    let fail_once = Arc::new(AtomicBool::new(false));
    let flag = fail_once.clone();
    let writer = registry.file.writer.clone();
    let journal_path = root
        .path()
        .join("projects/workspace-labels.transaction.json");
    Arc::get_mut(&mut registry.file).unwrap().writer = Arc::new(move |path, bytes| {
        if flag.swap(false, Ordering::SeqCst) {
            let journal: serde_json::Value =
                serde_json::from_slice(&std::fs::read(&journal_path).unwrap()).unwrap();
            assert_eq!(journal["beforeWorkspaces"][0]["updatedAt"], "latest-update");
            return Err(RegistryError::Io);
        }
        writer(path, bytes)
    });
    let mut previous = workspace("one");
    registry
        .upsert(&previous, WorkspaceMutationContext::default())
        .unwrap();
    let mut assignment = previous.clone();
    assignment.updated_at = "assignment".to_owned();
    assignment.labels = Some(vec!["Urgent".to_owned()]);
    previous.updated_at = "latest-update".to_owned();
    previous.title = Some("latest-title".to_owned());
    registry
        .upsert(&previous, WorkspaceMutationContext::default())
        .unwrap();
    let events = Arc::new(Mutex::new(Vec::new()));
    let captured = events.clone();
    let _listener = registry.subscribe_to_mutations(Arc::new(move |event| {
        captured.lock().unwrap().push(event.clone());
        Ok(())
    }));
    let store = FileWorkspaceLabelStore::new(root.path(), registry.clone());
    fail_once.store(true, Ordering::SeqCst);
    assert_eq!(
        store.commit(&WorkspaceLabelStoreMutation {
            require_active_workspace: None,
            expected_labels: Vec::new(),
            labels: vec![WorkspaceLabelDefinition {
                name: "Urgent".to_owned(),
                color: WorkspaceLabelColor::Red
            }],
            workspace_updates: vec![assignment],
        }),
        Err(WorkspaceLabelStoreError::Io)
    );
    assert_eq!(registry.get("one").unwrap(), Some(previous.clone()));
    assert!(events.lock().unwrap().is_empty());
    let reopened =
        FileWorkspaceLabelStore::new(root.path(), FileBackedWorkspaceRegistry::new(path));
    let snapshot = reopened.snapshot().unwrap();
    assert_eq!(snapshot.workspaces, vec![previous]);
    assert!(snapshot.labels.is_empty());
}
