use std::sync::{Arc, Mutex};

use crate::model::registry::{PersistedWorkspaceKind, UntrustedWorkspaceSource};
use crate::ports::registry::{
    MutationListener, MutationSubscription, WorkspaceArchiveContext, WorkspaceMutation,
    WorkspaceMutationContext,
};
use crate::ports::workspace_automation::{ScriptType, SetupLifecycle};

use super::*;

#[derive(Debug, Clone)]
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
            .find(|item| item.workspace_id == id)
            .cloned())
    }
    fn upsert(
        &self,
        _record: &PersistedWorkspaceRecord,
        _context: WorkspaceMutationContext,
    ) -> Result<(), RegistryError> {
        Ok(())
    }
    fn update(
        &self,
        id: &str,
        update: &dyn Fn(&PersistedWorkspaceRecord) -> PersistedWorkspaceRecord,
    ) -> Result<Option<PersistedWorkspaceRecord>, RegistryError> {
        let mut records = self.0.lock().expect("workspaces");
        let Some(record) = records.iter_mut().find(|item| item.workspace_id == id) else {
            return Ok(None);
        };
        *record = update(record);
        Ok(Some(record.clone()))
    }
    fn archive(
        &self,
        _id: &str,
        _timestamp: &str,
        _context: &WorkspaceArchiveContext,
    ) -> Result<(), RegistryError> {
        Ok(())
    }
    fn remove(&self, _id: &str) -> Result<(), RegistryError> {
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

#[derive(Debug, Clone, Default)]
struct Runtime {
    setups: Arc<Mutex<Vec<WorkspacePlacement>>>,
    scripts: Arc<Mutex<Vec<(String, String)>>>,
}

impl WorkspaceAutomationRuntime for Runtime {
    fn list_scripts(
        &self,
        workspace: &WorkspacePlacement,
    ) -> Result<Vec<ScriptSnapshot>, WorkspaceAutomationError> {
        Ok(vec![script(&workspace.workspace_id, false)])
    }
    fn start_script(
        &self,
        workspace: &WorkspacePlacement,
        script_name: &str,
    ) -> Result<ScriptSnapshot, WorkspaceAutomationError> {
        self.scripts
            .lock()
            .expect("scripts")
            .push((workspace.workspace_id.clone(), script_name.to_owned()));
        Ok(script(script_name, true))
    }
    fn stop_script(
        &self,
        _workspace: &WorkspacePlacement,
        script_name: &str,
    ) -> Result<ScriptSnapshot, WorkspaceAutomationError> {
        Ok(script(script_name, false))
    }
    fn start_setup(
        &self,
        workspace: &WorkspacePlacement,
    ) -> Result<bool, WorkspaceAutomationError> {
        self.setups.lock().expect("setups").push(workspace.clone());
        Ok(true)
    }
    fn setup_snapshot(&self, workspace_id: &str) -> Option<SetupSnapshot> {
        (workspace_id == "snapshot").then(|| SetupSnapshot {
            lifecycle: SetupLifecycle::Completed,
            worktree_path: "/repo".to_owned(),
            branch_name: "main".to_owned(),
            log: String::new(),
            commands: Vec::new(),
            truncated: false,
            error: None,
        })
    }
}

#[test]
fn status_prefers_runtime_and_derives_persisted_block() {
    let (service, _, _) = service();
    assert!(matches!(
        service.setup_status("snapshot"),
        Ok(SetupStatus::Snapshot(_))
    ));
    assert!(matches!(
        service.setup_status("blocked"),
        Ok(SetupStatus::Blocked { .. })
    ));
    assert_eq!(service.setup_status("missing"), Ok(SetupStatus::Absent));
}

#[test]
fn setup_approval_is_idempotent_and_clears_provenance_before_start() {
    let (service, workspaces, runtime) = service();
    assert_eq!(
        service.approve_and_start_setup("blocked", "later"),
        Ok(true)
    );
    assert_eq!(
        service.approve_and_start_setup("blocked", "later-2"),
        Ok(false)
    );
    let record = workspaces
        .get("blocked")
        .expect("registry")
        .expect("workspace");
    assert!(record.untrusted_source.is_none());
    assert_eq!(record.updated_at, "later");
    assert_eq!(runtime.setups.lock().expect("setups").len(), 1);
}

#[test]
fn scripts_require_active_trusted_workspace_and_forward_exact_name() {
    let (service, _, runtime) = service();
    assert!(service.start_script("blocked", "web").is_err());
    assert!(service.start_script("archived", "web").is_err());
    let started = service.start_script("trusted", "Web App").expect("start");
    assert!(started.running);
    assert_eq!(runtime.scripts.lock().expect("scripts")[0].1, "Web App");
    assert_eq!(service.list_scripts("trusted").expect("list").len(), 1);
    assert!(
        !service
            .stop_script("trusted", "Web App")
            .expect("stop")
            .running
    );
}

fn service() -> (WorkspaceAutomation, Workspaces, Runtime) {
    let workspaces = Workspaces(Arc::new(Mutex::new(vec![
        workspace("trusted", false, false),
        workspace("blocked", true, false),
        workspace("snapshot", false, false),
        workspace("archived", false, true),
    ])));
    let runtime = Runtime::default();
    (
        WorkspaceAutomation::new(Box::new(workspaces.clone()), Box::new(runtime.clone())),
        workspaces,
        runtime,
    )
}

fn workspace(id: &str, blocked: bool, archived: bool) -> PersistedWorkspaceRecord {
    PersistedWorkspaceRecord {
        workspace_id: id.to_owned(),
        project_id: "prj".to_owned(),
        cwd: "/repo".to_owned(),
        kind: PersistedWorkspaceKind::Directory,
        display_name: id.to_owned(),
        title: None,
        branch: Some("main".to_owned()),
        worktree_root: None,
        base_branch: None,
        is_paseo_owned_worktree: false,
        main_repo_root: None,
        created_at: "now".to_owned(),
        updated_at: "now".to_owned(),
        archived_at: archived.then(|| "then".to_owned()),
        auto_archived_change_request_url: None,
        pinned_at: None,
        labels: None,
        untrusted_source: blocked.then(|| UntrustedWorkspaceSource::ChangeRequest {
            forge: "github".to_owned(),
            number: 42,
            head_repository: "fork/repo".to_owned(),
        }),
    }
}

fn script(name: &str, running: bool) -> ScriptSnapshot {
    ScriptSnapshot {
        name: name.to_owned(),
        kind: ScriptType::Script,
        hostname: name.to_owned(),
        port: None,
        running,
        exit_code: None,
        terminal_id: running.then(|| "terminal-1".to_owned()),
    }
}
