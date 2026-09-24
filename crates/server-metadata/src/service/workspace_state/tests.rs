use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use crate::model::registry::PersistedWorkspaceKind;
use crate::ports::registry::{
    MutationListener, MutationSubscription, WorkspaceArchiveContext, WorkspaceMutation,
    WorkspaceMutationContext,
};
use crate::ports::workspace_state::WorkspaceAttentionChanges;

use super::*;

#[derive(Debug, Default)]
struct Calls {
    scans: AtomicUsize,
    cleared: Mutex<Vec<(String, String)>>,
    marked: Mutex<Vec<(String, String)>>,
}

#[derive(Debug)]
struct Attention {
    calls: Arc<Calls>,
    fail_scan: bool,
    fail_mark: Option<WorkspaceStateError>,
}

impl WorkspaceAttention for Attention {
    fn scan(&self) -> Result<Box<dyn WorkspaceAttentionScan + '_>, WorkspaceStateError> {
        self.calls.scans.fetch_add(1, Ordering::SeqCst);
        if self.fail_scan {
            return Err(WorkspaceStateError::AgentRegistry);
        }
        Ok(Box::new(Scan(self.calls.clone())))
    }

    fn mark_unread(
        &self,
        workspace_id: &str,
        updated_at: &str,
    ) -> Result<String, WorkspaceStateError> {
        self.calls
            .marked
            .lock()
            .unwrap()
            .push((workspace_id.to_owned(), updated_at.to_owned()));
        if let Some(error) = &self.fail_mark {
            return Err(error.clone());
        }
        Ok("selected-agent".to_owned())
    }
}

#[derive(Debug)]
struct Scan(Arc<Calls>);

impl WorkspaceAttentionScan for Scan {
    fn clear_attention(&self, workspace_id: &str, updated_at: &str) -> WorkspaceAttentionChanges {
        self.0
            .cleared
            .lock()
            .unwrap()
            .push((workspace_id.to_owned(), updated_at.to_owned()));
        WorkspaceAttentionChanges {
            cleared_agent_ids: vec![format!("agent-{workspace_id}")],
            error: (workspace_id == "partial").then_some(WorkspaceStateError::AgentRegistry),
        }
    }
}

#[derive(Debug, Clone, Default)]
struct Workspaces(Arc<Mutex<Vec<PersistedWorkspaceRecord>>>);

impl WorkspaceRegistry for Workspaces {
    fn initialize(&self) -> Result<(), RegistryError> {
        Ok(())
    }
    fn exists_on_disk(&self) -> bool {
        true
    }
    fn list(&self) -> Result<Vec<PersistedWorkspaceRecord>, RegistryError> {
        Ok(self.0.lock().expect("workspaces").clone())
    }
    fn get(&self, id: &str) -> Result<Option<PersistedWorkspaceRecord>, RegistryError> {
        Ok(self
            .0
            .lock()
            .expect("workspaces")
            .iter()
            .find(|record| record.workspace_id == id)
            .cloned())
    }
    fn upsert(
        &self,
        record: &PersistedWorkspaceRecord,
        _context: WorkspaceMutationContext,
    ) -> Result<(), RegistryError> {
        let mut records = self.0.lock().expect("workspaces");
        if let Some(current) = records
            .iter_mut()
            .find(|current| current.workspace_id == record.workspace_id)
        {
            current.clone_from(record);
        } else {
            records.push(record.clone());
        }
        Ok(())
    }
    fn update(
        &self,
        id: &str,
        update: &dyn Fn(&PersistedWorkspaceRecord) -> PersistedWorkspaceRecord,
    ) -> Result<Option<PersistedWorkspaceRecord>, RegistryError> {
        let mut records = self.0.lock().expect("workspaces");
        let Some(record) = records.iter_mut().find(|record| record.workspace_id == id) else {
            return Ok(None);
        };
        *record = update(record);
        Ok(Some(record.clone()))
    }
    fn archive(
        &self,
        id: &str,
        timestamp: &str,
        _context: &WorkspaceArchiveContext,
    ) -> Result<(), RegistryError> {
        self.update(id, &|record| {
            let mut record = record.clone();
            record.archived_at = Some(timestamp.to_owned());
            record
        })?;
        Ok(())
    }
    fn remove(&self, id: &str) -> Result<(), RegistryError> {
        self.0
            .lock()
            .expect("workspaces")
            .retain(|record| record.workspace_id != id);
        Ok(())
    }
    fn subscribe_to_mutations(
        &self,
        _listener: MutationListener<WorkspaceMutation>,
    ) -> Box<dyn MutationSubscription> {
        Box::new(Subscription)
    }
    fn block_all_mutations_until_restart(&self) -> Result<(), RegistryError> {
        Ok(())
    }
}

#[derive(Debug)]
struct Subscription;
impl MutationSubscription for Subscription {}

fn service(
    fail_scan: bool,
    fail_mark: Option<WorkspaceStateError>,
) -> (WorkspaceState, Arc<Calls>) {
    let calls = Arc::new(Calls::default());
    let attention = Attention {
        calls: calls.clone(),
        fail_scan,
        fail_mark,
    };
    let workspaces = Workspaces(Arc::new(Mutex::new(vec![
        workspace("one", true),
        workspace("partial", true),
        workspace("archived", false),
    ])));
    (
        WorkspaceState::new(Box::new(attention), Box::new(workspaces)),
        calls,
    )
}

#[test]
fn one_scan_preserves_order_duplicates_partial_commits_and_workspace_validation() {
    let (service, calls) = service(false, None);
    let result = service.clear_attention(
        &["one", "partial", "missing", "archived", "one"].map(str::to_owned),
        "now",
    );
    assert_eq!(calls.scans.load(Ordering::SeqCst), 1);
    assert_eq!(
        *calls.cleared.lock().unwrap(),
        [
            ("one".to_owned(), "now".to_owned()),
            ("partial".to_owned(), "now".to_owned()),
            ("one".to_owned(), "now".to_owned())
        ]
    );
    assert_eq!(
        result.cleared_agent_ids,
        ["agent-one", "agent-partial", "agent-one"]
    );
    assert_eq!(
        result
            .results
            .iter()
            .map(|result| result.success)
            .collect::<Vec<_>>(),
        [true, false, false, false, true]
    );
    assert_eq!(
        result.error.as_deref(),
        Some(
            "Agent runtime registry failed; Workspace not found: missing; Workspace not found: archived"
        )
    );
    assert!(!result.success);
}

#[test]
fn scan_failure_returns_one_failure_per_requested_workspace() {
    let (service, calls) = service(true, None);
    let result = service.clear_attention(&["one".to_owned(), "missing".to_owned()], "now");
    assert_eq!(calls.scans.load(Ordering::SeqCst), 1);
    assert!(calls.cleared.lock().unwrap().is_empty());
    assert!(result.cleared_agent_ids.is_empty());
    assert_eq!(result.results.len(), 2);
    assert!(result.results.iter().all(
        |item| !item.success && item.error.as_deref() == Some("Agent runtime registry failed")
    ));
}

#[test]
fn mark_unread_only_delegates_after_active_workspace_validation() {
    let (service, calls) = service(false, None);
    for workspace_id in ["missing", "archived"] {
        assert_eq!(
            service.mark_unread(workspace_id, "now"),
            Err(WorkspaceStateError::WorkspaceNotFound(
                workspace_id.to_owned()
            ))
        );
    }
    assert!(calls.marked.lock().unwrap().is_empty());
    assert_eq!(service.mark_unread("one", "now").unwrap(), "selected-agent");
    assert_eq!(
        *calls.marked.lock().unwrap(),
        [("one".to_owned(), "now".to_owned())]
    );
}

#[test]
fn mark_unread_keeps_agent_side_error_categories() {
    for error in [
        WorkspaceStateError::AgentRegistry,
        WorkspaceStateError::NoFinishedAgent("one".to_owned()),
        WorkspaceStateError::AgentNoLongerFinished("selected-agent".to_owned()),
    ] {
        let (service, _) = service(false, Some(error.clone()));
        assert_eq!(service.mark_unread("one", "now"), Err(error));
    }
}

#[test]
fn empty_batch_keeps_existing_success_and_snapshot_semantics() {
    let (service, calls) = service(false, None);
    let result = service.clear_attention(&[], "now");
    assert!(result.success);
    assert!(result.results.is_empty());
    assert!(result.error.is_none());
    assert_eq!(calls.scans.load(Ordering::SeqCst), 1);
}

fn workspace(id: &str, active: bool) -> PersistedWorkspaceRecord {
    PersistedWorkspaceRecord {
        workspace_id: id.to_owned(),
        project_id: "project".to_owned(),
        cwd: if id == "archived" {
            "/managed/feature/subdir".to_owned()
        } else {
            "/repo".to_owned()
        },
        kind: if id == "archived" {
            PersistedWorkspaceKind::Worktree
        } else {
            PersistedWorkspaceKind::Directory
        },
        display_name: id.to_owned(),
        title: None,
        branch: (id == "archived").then(|| "feature".to_owned()),
        worktree_root: (id == "archived").then(|| "/managed/feature".to_owned()),
        base_branch: (id == "archived").then(|| "main".to_owned()),
        is_paseo_owned_worktree: id == "archived",
        main_repo_root: (id == "archived").then(|| "/repo".to_owned()),
        created_at: "2026-09-22T09:00:00.000Z".to_owned(),
        updated_at: "2026-09-22T10:00:00.000Z".to_owned(),
        archived_at: (!active).then(|| "2026-09-22T11:00:00.000Z".to_owned()),
        auto_archived_change_request_url: None,
        pinned_at: None,
        labels: None,
        untrusted_source: None,
    }
}
