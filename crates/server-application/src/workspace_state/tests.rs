use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

use server_ports::registry::{
    ActiveProjectInput, MutationListener, MutationSubscription, ProjectMutation,
    WorkspaceArchiveContext, WorkspaceMutation, WorkspaceMutationContext,
};
use server_ports::workspace_recovery::{ArchivedWorktreeRestore, WorkspaceRecoveryRuntimeError};

use super::*;

#[derive(Debug, Clone, Default)]
struct Agents(Arc<Mutex<Vec<PersistedAgentRuntimeRecord>>>);

impl AgentRuntimeRegistry for Agents {
    fn initialize(&self) -> Result<(), AgentRuntimeRegistryError> {
        Ok(())
    }
    fn list(&self) -> Result<Vec<PersistedAgentRuntimeRecord>, AgentRuntimeRegistryError> {
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

#[derive(Debug, Clone, Default)]
struct Projects(Arc<Mutex<Vec<PersistedProjectRecord>>>);

impl ProjectRegistry for Projects {
    fn initialize(&self) -> Result<(), RegistryError> {
        Ok(())
    }
    fn exists_on_disk(&self) -> bool {
        true
    }
    fn list(&self) -> Result<Vec<PersistedProjectRecord>, RegistryError> {
        Ok(self.0.lock().expect("projects").clone())
    }
    fn get(&self, id: &str) -> Result<Option<PersistedProjectRecord>, RegistryError> {
        Ok(self
            .0
            .lock()
            .expect("projects")
            .iter()
            .find(|record| record.project_id == id)
            .cloned())
    }
    fn get_or_create_active_by_root(
        &self,
        _input: &ActiveProjectInput,
    ) -> Result<PersistedProjectRecord, RegistryError> {
        Err(RegistryError::InvalidRecord)
    }
    fn upsert(&self, record: &PersistedProjectRecord) -> Result<(), RegistryError> {
        let mut records = self.0.lock().expect("projects");
        if let Some(current) = records
            .iter_mut()
            .find(|current| current.project_id == record.project_id)
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
        update: &dyn Fn(&PersistedProjectRecord) -> PersistedProjectRecord,
    ) -> Result<Option<PersistedProjectRecord>, RegistryError> {
        let mut records = self.0.lock().expect("projects");
        let Some(record) = records.iter_mut().find(|record| record.project_id == id) else {
            return Ok(None);
        };
        *record = update(record);
        Ok(Some(record.clone()))
    }
    fn archive(&self, id: &str, timestamp: &str) -> Result<(), RegistryError> {
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
            .expect("projects")
            .retain(|record| record.project_id != id);
        Ok(())
    }
    fn subscribe_to_mutations(
        &self,
        _listener: MutationListener<ProjectMutation>,
    ) -> Box<dyn MutationSubscription> {
        Box::new(Subscription)
    }
}

#[derive(Debug)]
struct Subscription;
impl MutationSubscription for Subscription {}

#[derive(Debug, Clone, Default)]
struct Recovery {
    directories: Arc<Mutex<BTreeSet<String>>>,
    restores: Arc<Mutex<Vec<ArchivedWorktreeRestore>>>,
}

impl WorkspaceRecoveryRuntime for Recovery {
    fn is_directory(&self, path: &str) -> bool {
        self.directories.lock().expect("directories").contains(path)
    }

    fn restore_worktree(
        &self,
        input: &ArchivedWorktreeRestore,
    ) -> Result<(), WorkspaceRecoveryRuntimeError> {
        self.restores.lock().expect("restores").push(input.clone());
        Ok(())
    }
}

#[test]
fn clear_attention_processes_each_workspace_and_keeps_permission_attention() {
    let (service, agents, _, _, _) = service();
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
    let (service, agents, _, _, _) = service();
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
    let (service, agents, _, _, _) = service();
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

#[test]
fn recovery_inspection_distinguishes_unarchive_restore_and_unavailable() {
    let (service, _, workspaces, _, recovery) = service();
    recovery
        .directories
        .lock()
        .expect("directories")
        .extend(["/repo".to_owned(), "/existing".to_owned()]);
    workspaces.0.lock().expect("workspaces").extend([
        PersistedWorkspaceRecord {
            workspace_id: "existing".to_owned(),
            cwd: "/existing".to_owned(),
            archived_at: Some("2026-09-22T09:00:00.000Z".to_owned()),
            ..workspace("existing", false)
        },
        PersistedWorkspaceRecord {
            workspace_id: "missing-directory".to_owned(),
            kind: PersistedWorkspaceKind::Directory,
            cwd: "/gone".to_owned(),
            archived_at: Some("2026-09-22T09:00:00.000Z".to_owned()),
            ..workspace("missing-directory", false)
        },
    ]);

    assert!(matches!(
        service.inspect_recovery("existing").expect("inspect"),
        WorkspaceRecoveryState::Recoverable {
            action: WorkspaceRecoveryAction::Unarchive,
            ..
        }
    ));
    assert!(matches!(
        service.inspect_recovery("archived").expect("inspect"),
        WorkspaceRecoveryState::Recoverable {
            action: WorkspaceRecoveryAction::Restore,
            ..
        }
    ));
    assert!(matches!(
        service
            .inspect_recovery("missing-directory")
            .expect("inspect"),
        WorkspaceRecoveryState::Unavailable {
            reason: WorkspaceRecoveryUnavailableReason::WorkspaceDirectoryMissing,
            ..
        }
    ));
    assert!(matches!(
        service.inspect_recovery("unknown").expect("inspect"),
        WorkspaceRecoveryState::Unavailable {
            reason: WorkspaceRecoveryUnavailableReason::WorkspaceNotFound,
            ..
        }
    ));
}

#[test]
fn restore_recreates_saved_placement_and_unarchives_workspace_and_project() {
    let (service, _, workspaces, projects, recovery) = service();
    recovery
        .directories
        .lock()
        .expect("directories")
        .insert("/repo".to_owned());

    let restored = service
        .restore("archived", "2026-09-22T12:00:00.000Z")
        .expect("restore");

    assert_eq!(restored.action, WorkspaceRecoveryAction::Restore);
    assert!(restored.workspace.archived_at.is_none());
    assert!(restored.project.archived_at.is_none());
    assert_eq!(
        recovery.restores.lock().expect("restores").as_slice(),
        [ArchivedWorktreeRestore {
            source_repo_root: "/repo".to_owned(),
            previous_worktree_root: "/managed/feature".to_owned(),
            workspace_cwd: "/managed/feature/subdir".to_owned(),
            branch: "feature".to_owned(),
            base_ref: Some("main".to_owned()),
        }]
    );
    assert!(
        workspaces
            .get("archived")
            .expect("get")
            .expect("workspace")
            .archived_at
            .is_none()
    );
    assert!(
        projects
            .get("project")
            .expect("get")
            .expect("project")
            .archived_at
            .is_none()
    );
}

fn service() -> (WorkspaceState, Agents, Workspaces, Projects, Recovery) {
    let agents = Agents::default();
    let workspaces = Workspaces(Arc::new(Mutex::new(vec![
        workspace("wks-one", true),
        workspace("archived", false),
    ])));
    let projects = Projects(Arc::new(Mutex::new(vec![project()])));
    let recovery = Recovery::default();
    let service = WorkspaceState::new(
        Box::new(agents.clone()),
        Box::new(workspaces.clone()),
        Box::new(projects.clone()),
        Box::new(recovery.clone()),
    );
    (service, agents, workspaces, projects, recovery)
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

fn project() -> PersistedProjectRecord {
    PersistedProjectRecord {
        project_id: "project".to_owned(),
        root_path: "/repo".to_owned(),
        kind: server_domain::registry::PersistedProjectKind::Git,
        display_name: "Repo".to_owned(),
        project_key: Some("local:repo".to_owned()),
        custom_name: None,
        custom_icon_revision: None,
        created_at: "2026-09-22T09:00:00.000Z".to_owned(),
        updated_at: "2026-09-22T10:00:00.000Z".to_owned(),
        archived_at: Some("2026-09-22T11:00:00.000Z".to_owned()),
    }
}
