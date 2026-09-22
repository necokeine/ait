use std::fs;

use server_domain::registry::{PersistedWorkspaceKind, PersistedWorkspaceRecord};
use server_domain::workspace_labels::{WorkspaceLabelColor, WorkspaceLabelDefinition};
use server_ports::registry::{WorkspaceMutationContext, WorkspaceRegistry};
use server_ports::workspace_labels::{
    WorkspaceLabelStore, WorkspaceLabelStoreError, WorkspaceLabelStoreMutation,
};

use crate::registry::FileBackedWorkspaceRegistry;

use super::{
    FileWorkspaceLabelStore, Transaction, TransactionPhase, WorkspaceState, write_json_atomic,
};

fn workspace(labels: Option<Vec<String>>, updated_at: &str) -> PersistedWorkspaceRecord {
    PersistedWorkspaceRecord {
        workspace_id: "wks_one".to_owned(),
        project_id: "prj_one".to_owned(),
        cwd: "/repo".to_owned(),
        kind: PersistedWorkspaceKind::Directory,
        display_name: "repo".to_owned(),
        title: None,
        branch: None,
        worktree_root: None,
        base_branch: None,
        is_paseo_owned_worktree: false,
        main_repo_root: None,
        created_at: "2026-08-14T00:00:00.000Z".to_owned(),
        updated_at: updated_at.to_owned(),
        archived_at: None,
        auto_archived_change_request_url: None,
        pinned_at: None,
        labels,
        untrusted_source: None,
    }
}

fn urgent() -> WorkspaceLabelDefinition {
    WorkspaceLabelDefinition {
        name: "Urgent".to_owned(),
        color: WorkspaceLabelColor::Red,
    }
}

#[test]
fn compound_catalog_and_assignment_survive_reopen() {
    let temp = tempfile::tempdir().unwrap();
    let registry = FileBackedWorkspaceRegistry::new(temp.path().join("projects/workspaces.json"));
    registry
        .upsert(
            &workspace(None, "2026-08-14T00:00:00.000Z"),
            WorkspaceMutationContext::default(),
        )
        .unwrap();
    let store = FileWorkspaceLabelStore::new(temp.path(), registry.clone());
    let initial = store.snapshot().unwrap();
    store
        .commit(&WorkspaceLabelStoreMutation {
            expected_labels: initial.labels,
            labels: vec![urgent()],
            workspace_updates: vec![workspace(
                Some(vec!["Urgent".to_owned()]),
                "2026-08-14T01:00:00.000Z",
            )],
        })
        .unwrap();

    let reopened_registry =
        FileBackedWorkspaceRegistry::new(temp.path().join("projects/workspaces.json"));
    let reopened = FileWorkspaceLabelStore::new(temp.path(), reopened_registry);
    let snapshot = reopened.snapshot().unwrap();
    assert_eq!(snapshot.labels, vec![urgent()]);
    assert_eq!(
        snapshot.workspaces[0].labels,
        Some(vec!["Urgent".to_owned()])
    );
    assert!(
        !temp
            .path()
            .join("projects/workspace-labels.transaction.json")
            .exists()
    );
}

#[test]
fn prepared_transaction_rolls_both_files_back_on_restart() {
    let temp = tempfile::tempdir().unwrap();
    let projects = temp.path().join("projects");
    let registry = FileBackedWorkspaceRegistry::new(projects.join("workspaces.json"));
    registry
        .upsert(
            &workspace(Some(vec!["Urgent".to_owned()]), "2026-08-14T01:00:00.000Z"),
            WorkspaceMutationContext::default(),
        )
        .unwrap();
    write_json_atomic(&projects.join("workspace-labels.json"), &vec![urgent()]).unwrap();
    write_json_atomic(
        &projects.join("workspace-labels.transaction.json"),
        &Transaction {
            phase: TransactionPhase::Prepared,
            before_labels: Vec::new(),
            after_labels: vec![urgent()],
            before_workspaces: vec![WorkspaceState {
                workspace_id: "wks_one".to_owned(),
                labels: None,
                updated_at: "2026-08-14T00:00:00.000Z".to_owned(),
            }],
            after_workspaces: vec![WorkspaceState {
                workspace_id: "wks_one".to_owned(),
                labels: Some(vec!["Urgent".to_owned()]),
                updated_at: "2026-08-14T01:00:00.000Z".to_owned(),
            }],
        },
    )
    .unwrap();

    let store = FileWorkspaceLabelStore::new(temp.path(), registry);
    let snapshot = store.snapshot().unwrap();
    assert!(snapshot.labels.is_empty());
    assert_eq!(snapshot.workspaces[0].labels, None);
    assert_eq!(
        snapshot.workspaces[0].updated_at,
        "2026-08-14T00:00:00.000Z"
    );
}

#[test]
fn committed_marker_is_cleanup_state_and_does_not_replay_stale_workspaces() {
    let temp = tempfile::tempdir().unwrap();
    let projects = temp.path().join("projects");
    let current = workspace(Some(vec!["Current".to_owned()]), "2026-08-14T03:00:00.000Z");
    let registry = FileBackedWorkspaceRegistry::new(projects.join("workspaces.json"));
    registry
        .upsert(&current, WorkspaceMutationContext::default())
        .unwrap();
    let current_label = WorkspaceLabelDefinition {
        name: "Current".to_owned(),
        color: WorkspaceLabelColor::Blue,
    };
    write_json_atomic(
        &projects.join("workspace-labels.json"),
        &vec![current_label.clone()],
    )
    .unwrap();
    write_json_atomic(
        &projects.join("workspace-labels.transaction.json"),
        &Transaction {
            phase: TransactionPhase::Committed,
            before_labels: Vec::new(),
            after_labels: vec![urgent()],
            before_workspaces: Vec::new(),
            after_workspaces: vec![WorkspaceState {
                workspace_id: "wks_one".to_owned(),
                labels: Some(vec!["Urgent".to_owned()]),
                updated_at: "2026-08-14T01:00:00.000Z".to_owned(),
            }],
        },
    )
    .unwrap();

    let store = FileWorkspaceLabelStore::new(temp.path(), registry);
    let snapshot = store.snapshot().unwrap();
    assert_eq!(snapshot.labels, vec![current_label]);
    assert_eq!(snapshot.workspaces[0], current);
    assert!(!projects.join("workspace-labels.transaction.json").exists());
}

#[test]
fn unreadable_durable_outcome_blocks_further_mutations_until_restart() {
    let temp = tempfile::tempdir().unwrap();
    let projects = temp.path().join("projects");
    let registry = FileBackedWorkspaceRegistry::new(projects.join("workspaces.json"));
    registry
        .upsert(
            &workspace(None, "2026-08-14T00:00:00.000Z"),
            WorkspaceMutationContext::default(),
        )
        .unwrap();
    let store = FileWorkspaceLabelStore::new(temp.path(), registry);
    let initial = store.snapshot().unwrap();
    fs::create_dir(projects.join("workspace-labels.transaction.json")).unwrap();
    let result = store.commit(&WorkspaceLabelStoreMutation {
        expected_labels: initial.labels,
        labels: vec![urgent()],
        workspace_updates: vec![workspace(
            Some(vec!["Urgent".to_owned()]),
            "2026-08-14T01:00:00.000Z",
        )],
    });
    assert_eq!(result, Err(WorkspaceLabelStoreError::Uncertain));
    assert_eq!(store.snapshot(), Err(WorkspaceLabelStoreError::Uncertain));
}

#[test]
fn stale_catalog_and_invalid_documents_are_rejected() {
    let temp = tempfile::tempdir().unwrap();
    let projects = temp.path().join("projects");
    fs::create_dir_all(&projects).unwrap();
    fs::write(projects.join("workspace-labels.json"), b"not-json").unwrap();
    let registry = FileBackedWorkspaceRegistry::new(projects.join("workspaces.json"));
    let store = FileWorkspaceLabelStore::new(temp.path(), registry);
    assert_eq!(store.initialize(), Err(WorkspaceLabelStoreError::Invalid));

    fs::write(projects.join("workspace-labels.json"), b"[]").unwrap();
    let registry = FileBackedWorkspaceRegistry::new(projects.join("workspaces-2.json"));
    let store = FileWorkspaceLabelStore::new(temp.path(), registry);
    store.initialize().unwrap();
    assert_eq!(
        store.commit(&WorkspaceLabelStoreMutation {
            expected_labels: vec![urgent()],
            labels: Vec::new(),
            workspace_updates: Vec::new(),
        }),
        Err(WorkspaceLabelStoreError::Conflict)
    );
}
