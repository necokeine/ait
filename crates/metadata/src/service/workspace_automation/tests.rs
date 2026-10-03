use std::sync::{Arc, Mutex};

use crate::model::registry::{PersistedWorkspaceKind, UntrustedWorkspaceSource};
use crate::ports::registry::{
    MutationListener, MutationSubscription, WorkspaceArchiveContext, WorkspaceMutation,
    WorkspaceMutationContext,
};
use crate::ports::workspace_automation::{ScriptType, SetupLifecycle};

use super::*;

mod paseo;

#[test]
fn archive_cleanup_delegates_identities_even_after_workspace_records_are_archived() {
    let (service, _, runtime) = service();
    service
        .close_workspaces(&["archived".into(), "missing".into()])
        .unwrap();
    assert_eq!(*runtime.retired.lock().unwrap(), ["archived", "missing"]);
}

#[derive(Debug, Clone)]
struct Workspaces(Arc<Mutex<Vec<PersistedWorkspaceRecord>>>, Arc<Mutex<bool>>);

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
        if *self.1.lock().unwrap() {
            return Err(RegistryError::Io);
        }
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
    retired: Arc<Mutex<Vec<String>>>,
}

impl WorkspaceAutomationRuntime for Runtime {
    fn close_workspaces(&self, ids: &[String]) -> Result<(), WorkspaceAutomationError> {
        self.retired.lock().unwrap().extend_from_slice(ids);
        Ok(())
    }

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
fn rpc_reports_missing_workspaces_inline_and_keeps_absent_setup_distinct() {
    use crate::rpc::workspace_automation::execute;
    use serde_json::json;
    let (service, _, runtime) = service();
    let status = execute(
        &service,
        "workspace.setup.status.request",
        json!({"workspaceId":"missing"}),
    )
    .unwrap();
    assert!(status["snapshot"].is_null());
    for method in [
        "workspace.setup.run.request",
        "workspace.script.list.request",
        "workspace.script.start.request",
        "workspace.script.stop.request",
    ] {
        let result = execute(
            &service,
            method,
            json!({"workspaceId":"missing", "scriptName":"web"}),
        )
        .unwrap();
        assert_eq!(result["workspaceId"], "missing");
        assert!(
            result["error"]
                .as_str()
                .is_some_and(|error| error.contains("not found")),
            "{method}: {result}"
        );
    }
    assert!(runtime.setups.lock().unwrap().is_empty());
    assert!(runtime.scripts.lock().unwrap().is_empty());
    assert_eq!(
        execute(&service, "unknown", json!({})),
        Err(crate::rpc::ErrorCode::MethodNotFound)
    );
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

#[test]
fn committed_script_mutations_publish_complete_snapshots() {
    let (mut service, _, _) = service();
    let events = Arc::new(Mutex::new(Vec::new()));
    let capture = events.clone();
    service.set_event_sink(Arc::new(move |event| {
        capture.lock().expect("automation events").push(event);
    }));
    service.start_script("trusted", "Web App").expect("start");
    service.stop_script("trusted", "Web App").expect("stop");
    let events = events.lock().expect("automation events");
    assert_eq!(events.len(), 2);
    for event in events.iter() {
        let AutomationEvent::Scripts {
            workspace_id,
            scripts,
        } = event
        else {
            panic!("script update expected");
        };
        assert_eq!(workspace_id, "trusted");
        assert_eq!(scripts.len(), 1);
    }
}

fn service() -> (WorkspaceAutomation, Workspaces, Runtime) {
    let workspaces = Workspaces(
        Arc::new(Mutex::new(vec![
            workspace("trusted", false, false),
            workspace("blocked", true, false),
            workspace("snapshot", false, false),
            workspace("archived", false, true),
        ])),
        Arc::new(Mutex::new(false)),
    );
    let runtime = Runtime::default();
    (
        WorkspaceAutomation::new(Box::new(workspaces.clone()), Box::new(runtime.clone())),
        workspaces,
        runtime,
    )
}

fn workspace(id: &str, blocked: bool, archived: bool) -> PersistedWorkspaceRecord {
    PersistedWorkspaceRecord {
        auto_name: None,
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

#[test]
fn setup_status_reports_registry_failure_as_a_failed_snapshot_and_recovers_on_retry() {
    use crate::rpc::workspace_automation::execute;
    use serde_json::json;
    let (service, workspaces, runtime) = service();
    *workspaces.1.lock().unwrap() = true;
    let result = execute(
        &service,
        "workspace.setup.status.request",
        json!({"workspaceId":"trusted"}),
    )
    .unwrap();
    assert_eq!(result["snapshot"]["status"], "failed");
    assert!(result["snapshot"]["error"].is_string());
    assert_eq!(result["snapshot"]["detail"]["commands"], json!([]));
    assert!(runtime.setups.lock().unwrap().is_empty());
    *workspaces.1.lock().unwrap() = false;
    let retry = execute(
        &service,
        "workspace.setup.status.request",
        json!({"workspaceId":"trusted"}),
    )
    .unwrap();
    assert!(retry["snapshot"].is_null());
}
