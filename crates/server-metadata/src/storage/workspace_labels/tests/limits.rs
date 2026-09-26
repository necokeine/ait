use super::super::{MAX_DOCUMENT_BYTES, read_optional, transaction_for};
use super::*;

fn catalog_mutation(name: String) -> WorkspaceLabelStoreMutation {
    WorkspaceLabelStoreMutation {
        require_active_workspace: None,
        expected_labels: Vec::new(),
        labels: vec![WorkspaceLabelDefinition {
            name,
            color: WorkspaceLabelColor::Red,
        }],
        workspace_updates: Vec::new(),
    }
}

fn store(root: &std::path::Path) -> FileWorkspaceLabelStore {
    FileWorkspaceLabelStore::new(
        root,
        FileBackedWorkspaceRegistry::new(root.join("projects/workspaces.json")),
    )
}

#[test]
fn oversized_catalog_commit_is_rejected_and_the_empty_store_reopens() {
    let root = tempfile::tempdir().unwrap();
    let labels = store(root.path());
    let mutation = catalog_mutation("x".repeat(usize::try_from(MAX_DOCUMENT_BYTES).unwrap()));
    assert_eq!(
        labels.commit(&mutation),
        Err(WorkspaceLabelStoreError::Invalid)
    );
    assert!(labels.snapshot().unwrap().labels.is_empty());
    assert!(store(root.path()).snapshot().unwrap().labels.is_empty());
    assert!(
        !root
            .path()
            .join("projects/workspace-labels.transaction.json")
            .exists()
    );
    assert!(!root.path().join("projects/workspace-labels.json").exists());
}

#[test]
fn combined_journal_limit_preserves_the_previous_readable_catalog() {
    let root = tempfile::tempdir().unwrap();
    let labels = store(root.path());
    let first = catalog_mutation("a".repeat(usize::try_from(MAX_DOCUMENT_BYTES).unwrap() / 2));
    labels.commit(&first).unwrap();
    let path = root.path().join("projects/workspace-labels.json");
    let before = fs::read(&path).unwrap();
    let mut next = catalog_mutation("b".repeat(usize::try_from(MAX_DOCUMENT_BYTES).unwrap() / 2));
    next.expected_labels.clone_from(&first.labels);
    assert_eq!(labels.commit(&next), Err(WorkspaceLabelStoreError::Invalid));
    assert_eq!(fs::read(path).unwrap(), before);
    assert_eq!(store(root.path()).snapshot().unwrap().labels, first.labels);
    assert!(
        !root
            .path()
            .join("projects/workspace-labels.transaction.json")
            .exists()
    );
}

#[test]
fn committed_marker_size_is_checked_before_writing_a_prepared_marker() {
    let root = tempfile::tempdir().unwrap();
    let labels = store(root.path());
    let mut mutation = catalog_mutation("x".to_owned());
    let transaction = transaction_for(&[], &mutation, &[]).unwrap();
    let overhead = serde_json::to_vec_pretty(&transaction).unwrap().len();
    mutation.labels[0].name = "x".repeat(usize::try_from(MAX_DOCUMENT_BYTES).unwrap() - overhead);
    let prepared = transaction_for(&[], &mutation, &[]).unwrap();
    assert_eq!(
        serde_json::to_vec_pretty(&prepared).unwrap().len() + 1,
        usize::try_from(MAX_DOCUMENT_BYTES).unwrap()
    );
    assert_eq!(
        labels.commit(&mutation),
        Err(WorkspaceLabelStoreError::Invalid)
    );
    assert!(store(root.path()).snapshot().unwrap().labels.is_empty());
    assert!(
        !root
            .path()
            .join("projects/workspace-labels.transaction.json")
            .exists()
    );
}

#[test]
fn atomic_json_writer_accepts_the_read_limit_and_rejects_one_byte_more() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("document.json");
    let exact = "x".repeat(usize::try_from(MAX_DOCUMENT_BYTES).unwrap() - 3);
    write_json_atomic(&path, &exact).unwrap();
    assert_eq!(fs::metadata(&path).unwrap().len(), MAX_DOCUMENT_BYTES);
    assert_eq!(read_optional::<String>(&path).unwrap(), Some(exact));
    let previous = fs::read(&path).unwrap();
    assert_eq!(
        write_json_atomic(
            &path,
            &"x".repeat(usize::try_from(MAX_DOCUMENT_BYTES).unwrap() - 2)
        ),
        Err(WorkspaceLabelStoreError::Invalid)
    );
    assert_eq!(fs::read(path).unwrap(), previous);
}
