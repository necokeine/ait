use std::sync::{Arc, Mutex};

use crate::model::registry::{PersistedWorkspaceKind, PersistedWorkspaceRecord};
use crate::model::workspace_labels::{WorkspaceLabelColor, WorkspaceLabelDefinition};
use crate::ports::workspace_labels::{
    WorkspaceLabelStore, WorkspaceLabelStoreError, WorkspaceLabelStoreMutation,
    WorkspaceLabelStoreSnapshot,
};

use super::{
    WorkspaceLabelChange, WorkspaceLabelCursor, WorkspaceLabelError, WorkspaceLabelSyncMode,
    WorkspaceLabels,
};

mod admission;

#[derive(Debug)]
struct MemoryStore(Mutex<WorkspaceLabelStoreSnapshot>);

impl MemoryStore {
    fn new(workspaces: Vec<PersistedWorkspaceRecord>) -> Self {
        Self(Mutex::new(WorkspaceLabelStoreSnapshot {
            labels: Vec::new(),
            workspaces,
        }))
    }
}

impl WorkspaceLabelStore for MemoryStore {
    fn initialize(&self) -> Result<(), WorkspaceLabelStoreError> {
        Ok(())
    }

    fn snapshot(&self) -> Result<WorkspaceLabelStoreSnapshot, WorkspaceLabelStoreError> {
        Ok(self.0.lock().unwrap().clone())
    }

    fn commit(
        &self,
        mutation: &WorkspaceLabelStoreMutation,
    ) -> Result<(), WorkspaceLabelStoreError> {
        let mut state = self.0.lock().unwrap();
        if state.labels != mutation.expected_labels {
            return Err(WorkspaceLabelStoreError::Conflict);
        }
        state.labels.clone_from(&mutation.labels);
        for update in &mutation.workspace_updates {
            let workspace = state
                .workspaces
                .iter_mut()
                .find(|workspace| workspace.workspace_id == update.workspace_id)
                .ok_or(WorkspaceLabelStoreError::Invalid)?;
            workspace.clone_from(update);
        }
        Ok(())
    }
}

fn workspace(id: &str) -> PersistedWorkspaceRecord {
    PersistedWorkspaceRecord {
        workspace_id: id.to_owned(),
        project_id: "prj_one".to_owned(),
        cwd: format!("/repo/{id}"),
        kind: PersistedWorkspaceKind::Directory,
        display_name: id.to_owned(),
        title: None,
        branch: None,
        worktree_root: None,
        base_branch: None,
        is_paseo_owned_worktree: false,
        main_repo_root: None,
        created_at: "2026-08-14T00:00:00.000Z".to_owned(),
        updated_at: "2026-08-14T00:00:00.000Z".to_owned(),
        archived_at: None,
        auto_archived_change_request_url: None,
        pinned_at: None,
        labels: None,
        untrusted_source: None,
    }
}

fn label(name: &str, color: WorkspaceLabelColor) -> WorkspaceLabelDefinition {
    WorkspaceLabelDefinition {
        name: name.to_owned(),
        color,
    }
}

#[test]
fn normalizes_assignment_identity_and_preserves_the_catalog_definition() {
    let labels =
        WorkspaceLabels::new(Box::new(MemoryStore::new(vec![workspace("wks_one")]))).unwrap();
    let assigned = labels
        .set_assignment(
            "wks_one",
            &label("  Needs   review ", WorkspaceLabelColor::Sky),
            true,
            "2026-08-14T01:00:00.000Z",
        )
        .unwrap();
    assert_eq!(assigned.label.name, "Needs review");
    let existing = labels
        .set_assignment(
            "wks_one",
            &label("needs REVIEW", WorkspaceLabelColor::Red),
            true,
            "2026-08-14T02:00:00.000Z",
        )
        .unwrap();
    assert_eq!(existing.label, assigned.label);
    assert_eq!(labels.list(None).unwrap().labels, vec![assigned.label]);
}

#[test]
fn subscriptions_catch_up_and_publish_only_durable_catalog_changes() {
    let labels =
        WorkspaceLabels::new(Box::new(MemoryStore::new(vec![workspace("wks_one")]))).unwrap();
    let changes = Arc::new(Mutex::new(Vec::new()));
    let received = changes.clone();
    let (initial, _subscription) = labels
        .subscribe(
            None,
            Arc::new(move |change| received.lock().unwrap().push(change)),
        )
        .unwrap();
    labels
        .set_assignment(
            "wks_one",
            &label("Blocked", WorkspaceLabelColor::Red),
            true,
            "2026-08-14T01:00:00.000Z",
        )
        .unwrap();
    assert_eq!(changes.lock().unwrap().len(), 1);
    let caught_up = labels
        .list(Some(&WorkspaceLabelCursor {
            generation: initial.sync.generation,
            after_seq: initial.sync.head_seq,
        }))
        .unwrap();
    assert_eq!(caught_up.sync.mode, WorkspaceLabelSyncMode::Changes);
    assert_eq!(caught_up.labels[0].name, "Blocked");
}

#[test]
fn current_cursor_is_empty_and_an_expired_generation_gets_a_snapshot() {
    let labels =
        WorkspaceLabels::new(Box::new(MemoryStore::new(vec![workspace("wks_one")]))).unwrap();
    labels
        .set_assignment(
            "wks_one",
            &label("Blocked", WorkspaceLabelColor::Red),
            true,
            "2026-08-14T01:00:00.000Z",
        )
        .unwrap();
    let snapshot = labels.list(None).unwrap();
    let current = labels
        .list(Some(&WorkspaceLabelCursor {
            generation: snapshot.sync.generation,
            after_seq: snapshot.sync.head_seq,
        }))
        .unwrap();
    assert_eq!(current.sync.mode, WorkspaceLabelSyncMode::Changes);
    assert!(current.labels.is_empty());
    let expired = labels
        .list(Some(&WorkspaceLabelCursor {
            generation: "expired".to_owned(),
            after_seq: current.sync.head_seq,
        }))
        .unwrap();
    assert_eq!(expired.sync.mode, WorkspaceLabelSyncMode::Snapshot);
    assert_eq!(expired.labels[0].name, "Blocked");
}

#[test]
fn catch_up_compacts_name_cycles_and_chained_deletion() {
    let labels =
        WorkspaceLabels::new(Box::new(MemoryStore::new(vec![workspace("wks_one")]))).unwrap();
    labels
        .set_assignment(
            "wks_one",
            &label("A", WorkspaceLabelColor::Red),
            true,
            "2026-08-14T01:00:00.000Z",
        )
        .unwrap();
    let checkpoint = labels.list(None).unwrap().sync;
    labels
        .update("A", Some("B"), None, "2026-08-14T02:00:00.000Z")
        .unwrap();
    labels
        .update("B", Some("A"), None, "2026-08-14T03:00:00.000Z")
        .unwrap();
    let cycle = labels
        .list(Some(&WorkspaceLabelCursor {
            generation: checkpoint.generation,
            after_seq: checkpoint.head_seq,
        }))
        .unwrap();
    assert_eq!(cycle.labels, vec![label("A", WorkspaceLabelColor::Red)]);
    assert!(cycle.sync.removals.is_empty());
    labels
        .update("A", Some("B"), None, "2026-08-14T04:00:00.000Z")
        .unwrap();
    labels
        .update("B", Some("C"), None, "2026-08-14T05:00:00.000Z")
        .unwrap();
    labels.delete("C", "2026-08-14T06:00:00.000Z").unwrap();
    let deleted = labels
        .list(Some(&WorkspaceLabelCursor {
            generation: cycle.sync.generation,
            after_seq: cycle.sync.head_seq,
        }))
        .unwrap();
    assert!(deleted.labels.is_empty());
    assert_eq!(deleted.sync.removals[0].name, "A");
}

#[test]
fn no_op_assignment_and_released_subscription_stay_silent() {
    let labels =
        WorkspaceLabels::new(Box::new(MemoryStore::new(vec![workspace("wks_one")]))).unwrap();
    let changes = Arc::new(Mutex::new(Vec::new()));
    let received = changes.clone();
    let (_, subscription) = labels
        .subscribe(
            None,
            Arc::new(move |change| received.lock().unwrap().push(change)),
        )
        .unwrap();
    labels
        .set_assignment(
            "wks_one",
            &label("Quiet", WorkspaceLabelColor::Sky),
            false,
            "2026-08-14T01:00:00.000Z",
        )
        .unwrap();
    assert!(changes.lock().unwrap().is_empty());
    drop(subscription);
    labels
        .set_assignment(
            "wks_one",
            &label("Quiet", WorkspaceLabelColor::Sky),
            true,
            "2026-08-14T02:00:00.000Z",
        )
        .unwrap();
    assert!(changes.lock().unwrap().is_empty());
}

#[test]
fn one_edit_renames_and_recolors_or_rejects_the_whole_collision() {
    let labels =
        WorkspaceLabels::new(Box::new(MemoryStore::new(vec![workspace("wks_one")]))).unwrap();
    for definition in [
        label("Blocked", WorkspaceLabelColor::Red),
        label("Waiting", WorkspaceLabelColor::Amber),
    ] {
        labels
            .set_assignment("wks_one", &definition, true, "2026-08-14T01:00:00.000Z")
            .unwrap();
    }
    let edit = labels
        .update(
            "blocked",
            Some("Urgent"),
            Some(WorkspaceLabelColor::Sky),
            "2026-08-14T02:00:00.000Z",
        )
        .unwrap();
    assert_eq!(edit.label, label("Urgent", WorkspaceLabelColor::Sky));
    assert_eq!(edit.affected_workspace_count, 1);
    assert_eq!(
        labels.update(
            "Urgent",
            Some("waiting"),
            Some(WorkspaceLabelColor::Teal),
            "2026-08-14T03:00:00.000Z",
        ),
        Err(WorkspaceLabelError::NameTaken)
    );
    assert_eq!(labels.list(None).unwrap().labels[0], edit.label);
}

#[test]
fn case_only_edits_rewrite_assignments_and_exact_no_ops_stay_silent() {
    let labels =
        WorkspaceLabels::new(Box::new(MemoryStore::new(vec![workspace("wks_one")]))).unwrap();
    labels
        .set_assignment(
            "wks_one",
            &label("Urgent", WorkspaceLabelColor::Sky),
            true,
            "2026-08-14T01:00:00.000Z",
        )
        .unwrap();
    let changes = Arc::new(Mutex::new(Vec::new()));
    let received = changes.clone();
    let (_, _subscription) = labels
        .subscribe(
            None,
            Arc::new(move |change| received.lock().unwrap().push(change)),
        )
        .unwrap();
    let edited = labels
        .update("urgent", Some("URGENT"), None, "2026-08-14T02:00:00.000Z")
        .unwrap();
    assert_eq!(edited.affected_workspace_count, 1);
    assert_eq!(edited.label.name, "URGENT");
    assert_eq!(changes.lock().unwrap().len(), 1);
    let unchanged = labels
        .update(
            "URGENT",
            Some("URGENT"),
            Some(WorkspaceLabelColor::Sky),
            "2026-08-14T03:00:00.000Z",
        )
        .unwrap();
    assert_eq!(unchanged.affected_workspace_count, 0);
    assert_eq!(changes.lock().unwrap().len(), 1);
}

#[test]
fn delete_inspection_counts_archived_records_and_delete_is_idempotent() {
    let mut archived = workspace("wks_archived");
    archived.archived_at = Some("2026-08-14T01:00:00.000Z".to_owned());
    archived.labels = Some(vec!["Blocked".to_owned()]);
    let labels = WorkspaceLabels::new(Box::new(MemoryStore::new(vec![
        workspace("wks_one"),
        archived,
    ])))
    .unwrap();
    labels
        .set_assignment(
            "wks_one",
            &label("Blocked", WorkspaceLabelColor::Red),
            true,
            "2026-08-14T01:00:00.000Z",
        )
        .unwrap();
    assert_eq!(labels.inspect_delete("blocked").unwrap(), 2);
    assert_eq!(
        labels
            .delete("BLOCKED", "2026-08-14T02:00:00.000Z")
            .unwrap(),
        2
    );
    assert_eq!(
        labels
            .delete("Blocked", "2026-08-14T03:00:00.000Z")
            .unwrap(),
        0
    );
    assert!(labels.list(None).unwrap().labels.is_empty());
}

#[test]
fn update_publication_contains_the_previous_name() {
    let labels =
        WorkspaceLabels::new(Box::new(MemoryStore::new(vec![workspace("wks_one")]))).unwrap();
    labels
        .set_assignment(
            "wks_one",
            &label("A", WorkspaceLabelColor::Red),
            true,
            "2026-08-14T01:00:00.000Z",
        )
        .unwrap();
    let changes = Arc::new(Mutex::new(Vec::new()));
    let received = changes.clone();
    let (_, _subscription) = labels
        .subscribe(
            None,
            Arc::new(move |change| received.lock().unwrap().push(change)),
        )
        .unwrap();
    labels
        .update("A", Some("B"), None, "2026-08-14T02:00:00.000Z")
        .unwrap();
    assert!(matches!(
        &changes.lock().unwrap()[0].change,
        WorkspaceLabelChange::Upsert { previous_name: Some(name), .. } if name == "A"
    ));
}
