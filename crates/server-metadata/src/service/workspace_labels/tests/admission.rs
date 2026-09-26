use std::sync::atomic::{AtomicBool, Ordering};

use super::*;
use crate::ports::registry::{
    WorkspaceArchiveContext, WorkspaceMutationContext, WorkspaceRegistry,
};
use crate::storage::registry::FileBackedWorkspaceRegistry;
use crate::storage::workspace_labels::FileWorkspaceLabelStore;

#[derive(Debug)]
struct ArchiveAfterSnapshot {
    inner: FileWorkspaceLabelStore,
    registry: FileBackedWorkspaceRegistry,
    armed: Arc<AtomicBool>,
    remove_instead: bool,
}

impl WorkspaceLabelStore for ArchiveAfterSnapshot {
    fn initialize(&self) -> Result<(), WorkspaceLabelStoreError> {
        self.inner.initialize()
    }

    fn snapshot(&self) -> Result<WorkspaceLabelStoreSnapshot, WorkspaceLabelStoreError> {
        let snapshot = self.inner.snapshot()?;
        if self.armed.swap(false, Ordering::SeqCst) {
            if self.remove_instead {
                self.registry.remove("wks_one").unwrap();
            } else {
                self.registry
                    .archive(
                        "wks_one",
                        "concurrent-archive",
                        &WorkspaceArchiveContext::default(),
                    )
                    .unwrap();
            }
        }
        Ok(snapshot)
    }

    fn commit(
        &self,
        mutation: &WorkspaceLabelStoreMutation,
    ) -> Result<(), WorkspaceLabelStoreError> {
        self.inner.commit(mutation)
    }
}

struct Fixture {
    root: tempfile::TempDir,
    registry: FileBackedWorkspaceRegistry,
    labels: WorkspaceLabels,
    armed: Arc<AtomicBool>,
}

impl Fixture {
    fn new() -> Self {
        Self::with_removal(false)
    }

    fn with_removal(remove_instead: bool) -> Self {
        let root = tempfile::tempdir().unwrap();
        let registry =
            FileBackedWorkspaceRegistry::new(root.path().join("projects/workspaces.json"));
        registry
            .upsert(&workspace("wks_one"), WorkspaceMutationContext::default())
            .unwrap();
        let armed = Arc::new(AtomicBool::new(false));
        let labels = WorkspaceLabels::new(Box::new(ArchiveAfterSnapshot {
            inner: FileWorkspaceLabelStore::new(root.path(), registry.clone()),
            registry: registry.clone(),
            armed: armed.clone(),
            remove_instead,
        }))
        .unwrap();
        Self {
            root,
            registry,
            labels,
            armed,
        }
    }

    fn assign(
        &self,
        assigned: bool,
    ) -> Result<super::super::WorkspaceLabelAssignment, WorkspaceLabelError> {
        self.labels.set_assignment(
            "wks_one",
            &label("Urgent", WorkspaceLabelColor::Red),
            assigned,
            "assignment",
        )
    }
}

#[test]
fn assignment_reports_a_workspace_removed_after_its_planning_snapshot() {
    let fixture = Fixture::with_removal(true);
    fixture.armed.store(true, Ordering::SeqCst);
    assert_eq!(
        fixture.assign(true),
        Err(WorkspaceLabelError::WorkspaceNotFound)
    );
    assert!(fixture.registry.get("wks_one").unwrap().is_none());
    assert!(
        !fixture
            .root
            .path()
            .join("projects/workspace-labels.json")
            .exists()
    );
    assert!(fixture.labels.list(None).unwrap().labels.is_empty());
}

#[test]
fn assignment_rechecks_archival_after_snapshot_before_creating_a_catalog() {
    let fixture = Fixture::new();
    let changes = Arc::new(Mutex::new(Vec::new()));
    let observed = changes.clone();
    let (_, _subscription) = fixture
        .labels
        .subscribe(
            None,
            Arc::new(move |change| observed.lock().unwrap().push(change)),
        )
        .unwrap();
    fixture.armed.store(true, Ordering::SeqCst);
    assert_eq!(
        fixture.assign(true),
        Err(WorkspaceLabelError::WorkspaceNotFound)
    );
    let record = fixture.registry.get("wks_one").unwrap().unwrap();
    assert_eq!(record.updated_at, "concurrent-archive");
    assert_eq!(record.labels, None);
    assert!(changes.lock().unwrap().is_empty());
    assert!(
        !fixture
            .root
            .path()
            .join("projects/workspace-labels.json")
            .exists()
    );
}

#[test]
fn unassignment_rechecks_archival_and_preserves_the_existing_assignment() {
    let fixture = Fixture::new();
    fixture.assign(true).unwrap();
    fixture.armed.store(true, Ordering::SeqCst);
    assert_eq!(
        fixture.assign(false),
        Err(WorkspaceLabelError::WorkspaceNotFound)
    );
    let record = fixture.registry.get("wks_one").unwrap().unwrap();
    assert_eq!(record.updated_at, "concurrent-archive");
    assert_eq!(record.labels, Some(vec!["Urgent".to_owned()]));
}

#[test]
fn no_op_assignment_still_rechecks_the_current_archive_state() {
    let fixture = Fixture::new();
    fixture.assign(true).unwrap();
    fixture.armed.store(true, Ordering::SeqCst);
    assert_eq!(
        fixture.assign(true),
        Err(WorkspaceLabelError::WorkspaceNotFound)
    );
    assert_eq!(
        fixture.registry.get("wks_one").unwrap().unwrap().updated_at,
        "concurrent-archive"
    );
}

#[test]
fn catalog_rename_and_delete_still_rewrite_concurrently_archived_records() {
    let fixture = Fixture::new();
    fixture.assign(true).unwrap();
    fixture.armed.store(true, Ordering::SeqCst);
    assert_eq!(
        fixture
            .labels
            .update("Urgent", Some("Renamed"), None, "rename")
            .unwrap()
            .affected_workspace_count,
        1
    );
    let renamed = fixture.registry.get("wks_one").unwrap().unwrap();
    assert_eq!(renamed.archived_at.as_deref(), Some("concurrent-archive"));
    assert_eq!(renamed.labels, Some(vec!["Renamed".to_owned()]));
    assert_eq!(fixture.labels.delete("Renamed", "delete").unwrap(), 1);
    let deleted = fixture.registry.get("wks_one").unwrap().unwrap();
    assert_eq!(deleted.archived_at, renamed.archived_at);
    assert_eq!(deleted.labels, None);
}
