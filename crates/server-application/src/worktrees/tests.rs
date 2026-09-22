use std::path::Path;
use std::sync::{Arc, Mutex};

use server_ports::registry::{
    MutationListener, MutationSubscription, ProjectMutation, WorkspaceMutation,
};

use super::*;

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
            .find(|project| project.project_id == id)
            .cloned())
    }

    fn get_or_create_active_by_root(
        &self,
        input: &ActiveProjectInput,
    ) -> Result<PersistedProjectRecord, RegistryError> {
        let mut records = self.0.lock().expect("projects");
        if let Some(project) = records
            .iter()
            .find(|project| project.root_path == input.root_path && is_active_project(project))
        {
            return Ok(project.clone());
        }
        let project = PersistedProjectRecord {
            project_id: format!("prj_{}", records.len() + 1),
            root_path: input.root_path.clone(),
            kind: input.kind,
            display_name: input.display_name.clone(),
            project_key: input.project_key.clone(),
            custom_name: None,
            custom_icon_revision: None,
            created_at: input.timestamp.clone(),
            updated_at: input.timestamp.clone(),
            archived_at: None,
        };
        records.push(project.clone());
        Ok(project)
    }

    fn upsert(&self, record: &PersistedProjectRecord) -> Result<(), RegistryError> {
        let mut records = self.0.lock().expect("projects");
        if let Some(existing) = records
            .iter_mut()
            .find(|project| project.project_id == record.project_id)
        {
            existing.clone_from(record);
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
        let Some(record) = records.iter_mut().find(|project| project.project_id == id) else {
            return Ok(None);
        };
        *record = update(record);
        Ok(Some(record.clone()))
    }

    fn archive(&self, id: &str, timestamp: &str) -> Result<(), RegistryError> {
        self.update(id, &|project| {
            let mut project = project.clone();
            project.archived_at = Some(timestamp.to_owned());
            project
        })?;
        Ok(())
    }

    fn remove(&self, id: &str) -> Result<(), RegistryError> {
        self.0
            .lock()
            .expect("projects")
            .retain(|project| project.project_id != id);
        Ok(())
    }

    fn subscribe_to_mutations(
        &self,
        _listener: MutationListener<ProjectMutation>,
    ) -> Box<dyn MutationSubscription> {
        Box::new(Subscription)
    }
}

#[derive(Debug, Clone, Default)]
struct Workspaces {
    records: Arc<Mutex<Vec<PersistedWorkspaceRecord>>>,
    contexts: Arc<Mutex<Vec<WorkspaceMutationContext>>>,
    fail_upsert: Arc<Mutex<bool>>,
}

impl WorkspaceRegistry for Workspaces {
    fn initialize(&self) -> Result<(), RegistryError> {
        Ok(())
    }

    fn exists_on_disk(&self) -> bool {
        true
    }

    fn list(&self) -> Result<Vec<PersistedWorkspaceRecord>, RegistryError> {
        Ok(self.records.lock().expect("workspaces").clone())
    }

    fn get(&self, id: &str) -> Result<Option<PersistedWorkspaceRecord>, RegistryError> {
        Ok(self
            .records
            .lock()
            .expect("workspaces")
            .iter()
            .find(|workspace| workspace.workspace_id == id)
            .cloned())
    }

    fn upsert(
        &self,
        record: &PersistedWorkspaceRecord,
        context: WorkspaceMutationContext,
    ) -> Result<(), RegistryError> {
        if *self.fail_upsert.lock().expect("fail flag") {
            return Err(RegistryError::Io);
        }
        self.contexts.lock().expect("contexts").push(context);
        let mut records = self.records.lock().expect("workspaces");
        if let Some(existing) = records
            .iter_mut()
            .find(|workspace| workspace.workspace_id == record.workspace_id)
        {
            existing.clone_from(record);
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
        let mut records = self.records.lock().expect("workspaces");
        let Some(record) = records
            .iter_mut()
            .find(|workspace| workspace.workspace_id == id)
        else {
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
        self.update(id, &|workspace| {
            let mut workspace = workspace.clone();
            workspace.archived_at = Some(timestamp.to_owned());
            workspace
        })?;
        Ok(())
    }

    fn remove(&self, id: &str) -> Result<(), RegistryError> {
        self.records
            .lock()
            .expect("workspaces")
            .retain(|workspace| workspace.workspace_id != id);
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

#[derive(Debug, Clone)]
struct Managed {
    state: Arc<Mutex<ManagedState>>,
}

#[derive(Debug)]
struct ManagedState {
    listed: Vec<ManagedWorktreeInfo>,
    created_inputs: Vec<ManagedWorktreeCreate>,
    removed: Vec<String>,
    create_result: CreatedManagedWorktree,
}

impl Default for Managed {
    fn default() -> Self {
        Self {
            state: Arc::new(Mutex::new(ManagedState {
                listed: vec![ManagedWorktreeInfo {
                    path: "/managed/hash/topic".to_owned(),
                    created_at: "2026-01-01T00:00:00.000Z".to_owned(),
                    branch_name: Some("topic".to_owned()),
                    head: Some("a".repeat(40)),
                }],
                created_inputs: Vec::new(),
                removed: Vec::new(),
                create_result: CreatedManagedWorktree {
                    repo_root: "/repo".to_owned(),
                    source_cwd: "/repo/app".to_owned(),
                    workspace_cwd: "/managed/hash/topic/app".to_owned(),
                    worktree_path: "/managed/hash/topic".to_owned(),
                    branch_name: "topic".to_owned(),
                    comparison_base_ref: Some("refs/heads/main".to_owned()),
                    remote_url: Some("git@github.com:Acme/Repo.git".to_owned()),
                },
            })),
        }
    }
}

impl ManagedWorktrees for Managed {
    fn list(&self, _cwd: &str) -> Result<Vec<ManagedWorktreeInfo>, WorktreeError> {
        Ok(self.state.lock().expect("managed").listed.clone())
    }

    fn create(
        &self,
        input: &ManagedWorktreeCreate,
    ) -> Result<CreatedManagedWorktree, WorktreeError> {
        let mut state = self.state.lock().expect("managed");
        state.created_inputs.push(input.clone());
        Ok(state.create_result.clone())
    }

    fn owned(&self, path: &str) -> Result<OwnedWorktree, WorktreeError> {
        if self.contains("/managed/hash/topic", path) {
            Ok(OwnedWorktree {
                path: "/managed/hash/topic".to_owned(),
                repo_root: Some("/repo".to_owned()),
            })
        } else {
            Err(WorktreeError::NotAllowed)
        }
    }

    fn path_for_slug(&self, _repo_root: &str, slug: &str) -> Result<String, WorktreeError> {
        Ok(format!("/managed/hash/{slug}"))
    }

    fn contains(&self, root: &str, candidate: &str) -> bool {
        Path::new(candidate) == Path::new(root) || Path::new(candidate).starts_with(root)
    }

    fn remove(&self, worktree: &OwnedWorktree) -> Result<(), WorktreeError> {
        self.state
            .lock()
            .expect("managed")
            .removed
            .push(worktree.path.clone());
        Ok(())
    }
}

#[test]
fn create_reuses_source_project_and_records_paseo_placement() {
    let projects = Projects::default();
    projects
        .0
        .lock()
        .expect("projects")
        .push(project("prj_source"));
    let workspaces = Workspaces::default();
    workspaces
        .records
        .lock()
        .expect("workspaces")
        .push(workspace("source", "/repo/app", "/repo", "prj_source"));
    let managed = Managed::default();
    let service = service(&projects, &workspaces, &managed);

    let created = service
        .create(
            &CreateWorktree {
                cwd: "/repo/app".to_owned(),
                project_id: None,
                worktree_slug: Some(" Topic Name! ".to_owned()),
                ref_name: Some("main".to_owned()),
                action: CreateAction::BranchOff,
                has_change_request_source: false,
                first_agent_prompt: Some("\n  Review   the change carefully\nmore".to_owned()),
                expects_initial_agent: true,
            },
            "2026-01-02T00:00:00.000Z",
        )
        .expect("create workspace");

    assert_eq!(created.project.project_id, "prj_source");
    assert_eq!(created.workspace.kind, PersistedWorkspaceKind::Worktree);
    assert_eq!(created.workspace.cwd, "/managed/hash/topic/app");
    assert_eq!(
        created.workspace.worktree_root.as_deref(),
        Some("/managed/hash/topic")
    );
    assert_eq!(created.workspace.main_repo_root.as_deref(), Some("/repo"));
    assert!(created.workspace.is_paseo_owned_worktree);
    assert_eq!(
        created.workspace.title.as_deref(),
        Some("Review the change carefully")
    );
    assert_eq!(
        workspaces.contexts.lock().expect("contexts")[0].expects_initial_agent,
        Some(true)
    );
    let state = managed.state.lock().expect("managed");
    assert_eq!(state.created_inputs[0].slug, "topic-name");
    assert_eq!(
        state.created_inputs[0].mode,
        WorktreeCreateMode::BranchOff {
            base_ref: Some("main".to_owned()),
            branch_name: "topic-name".to_owned()
        }
    );
}

#[test]
fn missing_explicit_project_rolls_back_created_worktree() {
    let projects = Projects::default();
    let workspaces = Workspaces::default();
    let managed = Managed::default();
    let service = service(&projects, &workspaces, &managed);
    let error = service
        .create(&create_input(Some("missing")), "2026-01-02T00:00:00.000Z")
        .expect_err("missing project");
    assert_eq!(error, WorktreesError::UnknownProject("missing".to_owned()));
    assert_eq!(
        managed.state.lock().expect("managed").removed,
        ["/managed/hash/topic"]
    );
}

#[test]
fn registry_write_failure_rolls_back_created_worktree() {
    let projects = Projects::default();
    let workspaces = Workspaces::default();
    *workspaces.fail_upsert.lock().expect("fail flag") = true;
    let managed = Managed::default();
    let service = service(&projects, &workspaces, &managed);
    assert_eq!(
        service.create(&create_input(None), "2026-01-02T00:00:00.000Z"),
        Err(WorktreesError::Registry)
    );
    assert_eq!(
        managed.state.lock().expect("managed").removed,
        ["/managed/hash/topic"]
    );
}

#[test]
fn workspace_scope_keeps_worktree_until_last_active_reference() {
    let projects = Projects::default();
    let workspaces = Workspaces::default();
    workspaces.records.lock().expect("workspaces").extend([
        workspace("one", "/managed/hash/topic", "/managed/hash/topic", "prj"),
        workspace("two", "/managed/hash/topic", "/managed/hash/topic", "prj"),
    ]);
    let managed = Managed::default();
    let service = service(&projects, &workspaces, &managed);
    let first = service
        .archive(
            &ArchiveWorktree {
                worktree_path: Some("/managed/hash/topic".to_owned()),
                repo_root: None,
                worktree_slug: None,
                branch_name: None,
                workspace_id: Some("one".to_owned()),
                scope: ArchiveScope::Workspace,
            },
            "2026-01-03T00:00:00.000Z",
        )
        .expect("first archive");
    assert_eq!(first.workspace_ids, ["one"]);
    assert!(managed.state.lock().expect("managed").removed.is_empty());

    let second = service
        .archive(
            &ArchiveWorktree {
                workspace_id: Some("two".to_owned()),
                ..archive_input(ArchiveScope::Workspace)
            },
            "2026-01-03T00:00:01.000Z",
        )
        .expect("second archive");
    assert_eq!(second.workspace_ids, ["two"]);
    assert_eq!(
        managed.state.lock().expect("managed").removed,
        ["/managed/hash/topic"]
    );
}

#[test]
fn explicit_workspace_archive_uses_the_records_backing_worktree() {
    let projects = Projects::default();
    let workspaces = Workspaces::default();
    workspaces
        .records
        .lock()
        .expect("workspaces")
        .push(workspace(
            "one",
            "/managed/hash/topic/app",
            "/managed/hash/topic",
            "prj",
        ));
    let managed = Managed::default();
    let service = service(&projects, &workspaces, &managed);

    let archived = service
        .archive(
            &ArchiveWorktree {
                worktree_path: Some("/external/unrelated".to_owned()),
                workspace_id: Some("one".to_owned()),
                ..archive_input(ArchiveScope::Workspace)
            },
            "2026-01-03T00:00:00.000Z",
        )
        .expect("explicit archive");

    assert_eq!(archived.workspace_ids, ["one"]);
    assert_eq!(
        managed.state.lock().expect("managed").removed,
        ["/managed/hash/topic"]
    );
}

#[test]
fn workspace_scope_does_not_delete_a_directory_workspace_inside_managed_root() {
    let projects = Projects::default();
    let workspaces = Workspaces::default();
    let mut record = workspace(
        "directory",
        "/managed/hash/topic",
        "/managed/hash/topic",
        "prj",
    );
    record.kind = PersistedWorkspaceKind::Directory;
    record.is_paseo_owned_worktree = false;
    record.worktree_root = None;
    record.main_repo_root = None;
    workspaces.records.lock().expect("workspaces").push(record);
    let managed = Managed::default();
    let service = service(&projects, &workspaces, &managed);

    let archived = service
        .archive(
            &archive_input(ArchiveScope::Workspace),
            "2026-01-03T00:00:00.000Z",
        )
        .expect("directory archive");

    assert_eq!(archived.workspace_ids, ["directory"]);
    assert!(managed.state.lock().expect("managed").removed.is_empty());
}

#[test]
fn repeated_workspace_archive_removes_residual_owned_worktree() {
    let projects = Projects::default();
    let workspaces = Workspaces::default();
    let mut record = workspace(
        "archived",
        "/managed/hash/topic",
        "/managed/hash/topic",
        "prj",
    );
    record.archived_at = Some("2026-01-02T00:00:00.000Z".to_owned());
    workspaces.records.lock().expect("workspaces").push(record);
    let managed = Managed::default();
    let service = service(&projects, &workspaces, &managed);

    let archived = service
        .archive(
            &ArchiveWorktree {
                workspace_id: Some("archived".to_owned()),
                ..archive_input(ArchiveScope::Workspace)
            },
            "2026-01-03T00:00:00.000Z",
        )
        .expect("repeated archive");

    assert!(archived.workspace_ids.is_empty());
    assert_eq!(
        managed.state.lock().expect("managed").removed,
        ["/managed/hash/topic"]
    );
}

#[test]
fn worktree_scope_archives_all_descendant_workspaces_and_removes_directory() {
    let projects = Projects::default();
    let workspaces = Workspaces::default();
    workspaces.records.lock().expect("workspaces").extend([
        workspace("one", "/managed/hash/topic", "/managed/hash/topic", "prj"),
        workspace(
            "two",
            "/managed/hash/topic/packages/app",
            "/managed/hash/topic",
            "prj",
        ),
        workspace("other", "/repo", "/repo", "prj"),
    ]);
    let managed = Managed::default();
    let service = service(&projects, &workspaces, &managed);
    let archived = service
        .archive(
            &archive_input(ArchiveScope::Worktree),
            "2026-01-03T00:00:00.000Z",
        )
        .expect("worktree archive");
    assert_eq!(archived.workspace_ids, ["one", "two"]);
    assert_eq!(
        managed.state.lock().expect("managed").removed,
        ["/managed/hash/topic"]
    );
    let active = workspaces
        .list()
        .expect("list")
        .into_iter()
        .filter(is_active)
        .map(|workspace| workspace.workspace_id)
        .collect::<Vec<_>>();
    assert_eq!(active, ["other"]);
}

#[test]
fn worktree_scope_rejects_non_owned_target() {
    let projects = Projects::default();
    let workspaces = Workspaces::default();
    let managed = Managed::default();
    let service = service(&projects, &workspaces, &managed);
    let error = service
        .archive(
            &ArchiveWorktree {
                worktree_path: Some("/external/worktree".to_owned()),
                ..archive_input(ArchiveScope::Worktree)
            },
            "2026-01-03T00:00:00.000Z",
        )
        .expect_err("external archive");
    assert_eq!(error.kind(), WorktreeFailureKind::NotAllowed);
}

#[test]
fn checkout_requires_target_before_calling_adapter() {
    let projects = Projects::default();
    let workspaces = Workspaces::default();
    let managed = Managed::default();
    let service = service(&projects, &workspaces, &managed);
    let mut input = create_input(None);
    input.action = CreateAction::Checkout;
    input.ref_name = None;
    let error = service
        .create(&input, "2026-01-02T00:00:00.000Z")
        .expect_err("missing checkout target");
    assert_eq!(error.kind(), WorktreeFailureKind::MissingCheckoutTarget);
    assert!(
        managed
            .state
            .lock()
            .expect("managed")
            .created_inputs
            .is_empty()
    );
}

fn service(projects: &Projects, workspaces: &Workspaces, managed: &Managed) -> Worktrees {
    Worktrees::new(
        Box::new(projects.clone()),
        Box::new(workspaces.clone()),
        Box::new(managed.clone()),
        "server-test".to_owned(),
    )
}

fn create_input(project_id: Option<&str>) -> CreateWorktree {
    CreateWorktree {
        cwd: "/repo/app".to_owned(),
        project_id: project_id.map(str::to_owned),
        worktree_slug: Some("topic".to_owned()),
        ref_name: Some("main".to_owned()),
        action: CreateAction::BranchOff,
        has_change_request_source: false,
        first_agent_prompt: None,
        expects_initial_agent: false,
    }
}

fn archive_input(scope: ArchiveScope) -> ArchiveWorktree {
    ArchiveWorktree {
        worktree_path: Some("/managed/hash/topic".to_owned()),
        repo_root: None,
        worktree_slug: None,
        branch_name: None,
        workspace_id: None,
        scope,
    }
}

fn project(id: &str) -> PersistedProjectRecord {
    PersistedProjectRecord {
        project_id: id.to_owned(),
        root_path: "/repo".to_owned(),
        kind: PersistedProjectKind::Git,
        display_name: "repo".to_owned(),
        project_key: Some("remote:github.com/acme/repo".to_owned()),
        custom_name: None,
        custom_icon_revision: None,
        created_at: "2026-01-01T00:00:00.000Z".to_owned(),
        updated_at: "2026-01-01T00:00:00.000Z".to_owned(),
        archived_at: None,
    }
}

fn workspace(
    id: &str,
    cwd: &str,
    worktree_root: &str,
    project_id: &str,
) -> PersistedWorkspaceRecord {
    PersistedWorkspaceRecord {
        workspace_id: id.to_owned(),
        project_id: project_id.to_owned(),
        cwd: cwd.to_owned(),
        kind: PersistedWorkspaceKind::Worktree,
        display_name: id.to_owned(),
        title: None,
        branch: Some("topic".to_owned()),
        worktree_root: Some(worktree_root.to_owned()),
        base_branch: Some("refs/heads/main".to_owned()),
        is_paseo_owned_worktree: true,
        main_repo_root: Some("/repo".to_owned()),
        created_at: "2026-01-01T00:00:00.000Z".to_owned(),
        updated_at: "2026-01-01T00:00:00.000Z".to_owned(),
        archived_at: None,
        auto_archived_change_request_url: None,
        pinned_at: None,
        labels: None,
        untrusted_source: None,
    }
}
