use server_metadata::model::registry::PersistedWorkspaceKind;
use server_metadata::model::registry::PersistedWorkspaceRecord;
use server_metadata::ports::registry::{RegistryError, WorkspaceRegistry};
use server_metadata::service::workspace_state::WorkspaceState;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use server_metadata::ports::registry::{
    MutationListener, MutationSubscription, WorkspaceArchiveContext, WorkspaceMutation,
    WorkspaceMutationContext,
};

use super::*;

#[derive(Debug, Clone, Default)]
struct Agents(
    Arc<Mutex<Vec<PersistedAgentRuntimeRecord>>>,
    Arc<AtomicUsize>,
    Arc<Mutex<Faults>>,
);

#[derive(Debug, Default)]
struct Faults {
    list: bool,
    update: Option<UpdateFailure>,
}

#[derive(Debug, Clone, Copy)]
enum UpdateFailure {
    Io,
    Missing,
    Running,
}

impl AgentRuntimeRegistry for Agents {
    fn initialize(&self) -> Result<(), AgentRuntimeRegistryError> {
        Ok(())
    }
    fn list(&self) -> Result<Vec<PersistedAgentRuntimeRecord>, AgentRuntimeRegistryError> {
        self.1.fetch_add(1, Ordering::SeqCst);
        if self.2.lock().unwrap().list {
            return Err(AgentRuntimeRegistryError::Io);
        }
        Ok(self.0.lock().expect("agents").clone())
    }
    fn get(
        &self,
        agent_id: &str,
    ) -> Result<Option<PersistedAgentRuntimeRecord>, AgentRuntimeRegistryError> {
        Ok(self
            .0
            .lock()
            .expect("agents")
            .iter()
            .find(|record| record.id == agent_id)
            .cloned())
    }
    fn upsert(
        &self,
        record: &PersistedAgentRuntimeRecord,
    ) -> Result<(), AgentRuntimeRegistryError> {
        let mut records = self.0.lock().expect("agents");
        if let Some(current) = records.iter_mut().find(|current| current.id == record.id) {
            current.clone_from(record);
        } else {
            records.push(record.clone());
        }
        Ok(())
    }
    fn update(
        &self,
        agent_id: &str,
        update: &dyn Fn(&PersistedAgentRuntimeRecord) -> PersistedAgentRuntimeRecord,
    ) -> Result<Option<PersistedAgentRuntimeRecord>, AgentRuntimeRegistryError> {
        let mut records = self.0.lock().expect("agents");
        let Some(record) = records.iter_mut().find(|record| record.id == agent_id) else {
            return Ok(None);
        };
        if agent_id == "changed" {
            match self.2.lock().unwrap().update {
                Some(UpdateFailure::Io) => return Err(AgentRuntimeRegistryError::Io),
                Some(UpdateFailure::Missing) => return Ok(None),
                Some(UpdateFailure::Running) => record.last_status = AgentRuntimeStatus::Running,
                None => {}
            }
        }
        *record = update(record);
        Ok(Some(record.clone()))
    }
    fn remove(&self, agent_id: &str) -> Result<bool, AgentRuntimeRegistryError> {
        let mut records = self.0.lock().expect("agents");
        let before = records.len();
        records.retain(|record| record.id != agent_id);
        Ok(records.len() != before)
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

#[test]
fn clear_attention_processes_each_workspace_and_keeps_permission_attention() {
    let (service, agents, _) = service();
    agents.0.lock().expect("agents").extend([
        agent("finished", "wks-one", "2026-09-22T10:00:00.000Z"),
        PersistedAgentRuntimeRecord {
            id: "permission".to_owned(),
            attention_reason: Some(AgentAttentionReason::Permission),
            ..agent("permission", "wks-one", "2026-09-22T10:00:00.000Z")
        },
    ]);

    let result = service.clear_attention(
        &["wks-one".to_owned(), "missing".to_owned()],
        "2026-09-22T11:00:00.000Z",
    );

    assert!(!result.success);
    assert_eq!(result.cleared_agent_ids, ["finished"]);
    assert_eq!(result.results[0].cleared_agent_ids, ["finished"]);
    assert!(result.results[0].success);
    assert_eq!(
        result.results[1].error.as_deref(),
        Some("Workspace not found: missing")
    );
    assert!(
        agents
            .get("permission")
            .expect("get")
            .expect("permission")
            .requires_attention
    );
}

#[test]
fn mark_unread_selects_the_finished_root_instead_of_its_newer_child() {
    let (service, agents, _) = service();
    agents.0.lock().expect("agents").extend([
        PersistedAgentRuntimeRecord {
            requires_attention: false,
            ..agent("root", "wks-one", "2026-09-22T10:00:00.000Z")
        },
        PersistedAgentRuntimeRecord {
            id: "child".to_owned(),
            updated_at: "2026-09-22T12:00:00.000Z".to_owned(),
            requires_attention: false,
            labels: BTreeMap::from([(PARENT_AGENT_ID_LABEL.to_owned(), "root".to_owned())]),
            ..agent("child", "wks-one", "2026-09-22T12:00:00.000Z")
        },
    ]);

    let marked = service
        .mark_unread("wks-one", "2026-09-22T11:00:00.000Z")
        .expect("mark unread");

    assert_eq!(marked, "root");
    let root = agents.get("root").expect("get").expect("root");
    assert!(root.requires_attention);
    assert_eq!(root.attention_reason, Some(AgentAttentionReason::Finished));
    assert_eq!(
        root.attention_timestamp.as_deref(),
        Some("2026-09-22T11:00:00.000Z")
    );
    assert!(
        !agents
            .get("child")
            .expect("get")
            .expect("child")
            .requires_attention
    );
}

#[test]
fn mark_unread_rejects_a_workspace_without_a_finished_read_root() {
    let (service, agents, _) = service();
    agents
        .0
        .lock()
        .expect("agents")
        .push(PersistedAgentRuntimeRecord {
            last_status: AgentRuntimeStatus::Running,
            requires_attention: false,
            ..agent("running", "wks-one", "2026-09-22T10:00:00.000Z")
        });

    assert_eq!(
        service.mark_unread("wks-one", "2026-09-22T11:00:00.000Z"),
        Err(WorkspaceStateError::NoFinishedAgent("wks-one".to_owned()))
    );
}

fn service() -> (WorkspaceState, Agents, Workspaces) {
    let agents = Agents::default();
    let workspaces = Workspaces(Arc::new(Mutex::new(vec![
        workspace("wks-one", true),
        workspace("archived", false),
    ])));
    let service = WorkspaceState::new(
        Box::new(AgentWorkspaceAttention::new(Box::new(agents.clone()))),
        Box::new(workspaces.clone()),
    );
    (service, agents, workspaces)
}

fn agent(id: &str, workspace_id: &str, updated_at: &str) -> PersistedAgentRuntimeRecord {
    PersistedAgentRuntimeRecord {
        id: id.to_owned(),
        provider: "codex".to_owned(),
        cwd: "/repo".to_owned(),
        workspace_id: Some(workspace_id.to_owned()),
        created_at: "2026-09-22T09:00:00.000Z".to_owned(),
        updated_at: updated_at.to_owned(),
        last_activity_at: None,
        last_user_message_at: None,
        title: None,
        labels: BTreeMap::new(),
        last_status: AgentRuntimeStatus::Closed,
        last_mode_id: None,
        config: None,
        runtime_info: None,
        features: Vec::new(),
        persistence: None,
        last_error: None,
        requires_attention: true,
        attention_reason: Some(AgentAttentionReason::Finished),
        attention_timestamp: Some(updated_at.to_owned()),
        internal: false,
        archived_at: None,
        owner: None,
    }
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

#[test]
fn clear_batch_reads_agents_once_and_keeps_updates_before_a_failure() {
    for failure in [UpdateFailure::Io, UpdateFailure::Missing] {
        let (service, agents, workspaces) = service();
        workspaces
            .0
            .lock()
            .unwrap()
            .push(workspace("wks-two", true));
        agents.0.lock().unwrap().extend([
            agent("first", "wks-one", "2026-09-22T10:00:00.000Z"),
            agent("changed", "wks-one", "2026-09-22T10:00:00.000Z"),
            agent("third", "wks-one", "2026-09-22T10:00:00.000Z"),
            agent("second-workspace", "wks-two", "2026-09-22T10:00:00.000Z"),
        ]);
        agents.2.lock().unwrap().update = Some(failure);
        let result = service.clear_attention(
            &["wks-one".to_owned(), "wks-two".to_owned()],
            "2026-09-22T10:00:00.000Z",
        );
        assert_eq!(agents.1.load(Ordering::SeqCst), 1);
        assert_eq!(result.cleared_agent_ids, ["first", "second-workspace"]);
        assert!(!result.results[0].success);
        assert!(result.results[1].success);
        assert!(agents.get("third").unwrap().unwrap().requires_attention);
        let first = agents.get("first").unwrap().unwrap();
        assert!(!first.requires_attention);
        assert_eq!(first.updated_at, "2026-09-22T10:00:00.001Z");
    }
}

#[test]
fn attention_adapter_reports_scan_and_mark_storage_failures() {
    let (service, agents, _) = service();
    agents.2.lock().unwrap().list = true;
    let result = service.clear_attention(&["wks-one".to_owned()], "now");
    assert!(!result.success);
    assert_eq!(
        result.results[0].error.as_deref(),
        Some("Agent runtime registry failed")
    );
    assert_eq!(
        service.mark_unread("wks-one", "now"),
        Err(WorkspaceStateError::AgentRegistry)
    );
}

#[test]
fn mark_unread_rechecks_the_candidate_at_update_and_reports_lost_records() {
    for failure in [
        UpdateFailure::Running,
        UpdateFailure::Missing,
        UpdateFailure::Io,
    ] {
        let (service, agents, _) = service();
        let mut record = agent("changed", "wks-one", "2026-09-22T10:00:00.000Z");
        record.requires_attention = false;
        agents.0.lock().unwrap().push(record);
        agents.2.lock().unwrap().update = Some(failure);
        let expected = match failure {
            UpdateFailure::Io => WorkspaceStateError::AgentRegistry,
            UpdateFailure::Running | UpdateFailure::Missing => {
                WorkspaceStateError::AgentNoLongerFinished("changed".to_owned())
            }
        };
        assert_eq!(
            service.mark_unread("wks-one", "2026-09-22T11:00:00.000Z"),
            Err(expected)
        );
        assert!(!agents.get("changed").unwrap().unwrap().requires_attention);
    }
}

#[test]
fn attention_scan_does_not_clear_internal_archived_read_or_other_workspace_agents() {
    let (service, agents, _) = service();
    let mut internal = agent("internal", "wks-one", "now");
    internal.internal = true;
    let mut archived = agent("archived", "wks-one", "now");
    archived.archived_at = Some("yesterday".to_owned());
    let mut read = agent("read", "wks-one", "now");
    read.requires_attention = false;
    agents
        .0
        .lock()
        .unwrap()
        .extend([internal, archived, read, agent("other", "wks-two", "now")]);
    let result = service.clear_attention(&["wks-one".to_owned()], "later");
    assert!(result.success);
    assert!(result.cleared_agent_ids.is_empty());
}
