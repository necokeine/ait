use std::sync::Mutex;

use crate::model::registry::{
    PersistedProjectKind, PersistedProjectRecord, PersistedWorkspaceKind, PersistedWorkspaceRecord,
};
use crate::ports::provisioning::{Checkout, DirectorySource, DirectorySourceError};
use crate::ports::provisioning::{
    ProjectConfigDocument, ProjectConfigRevision as StoreConfigRevision, ProjectConfigStore,
    ProjectConfigStoreError, ProjectConfigWrite, ProjectIcon, ProjectIconStore,
    ProjectIconStoreError,
};
use crate::ports::registry::{
    ActiveProjectInput, MutationListener, MutationSubscription, ProjectMutation, ProjectRegistry,
    RegistryError, WorkspaceArchiveContext, WorkspaceMutation, WorkspaceMutationContext,
    WorkspaceRegistry,
};

use super::{Directory, DirectoryDependencies, DirectoryError, derive_project_key};

mod paseo;

#[derive(Debug, Default)]
struct Projects(Mutex<Vec<PersistedProjectRecord>>);

impl ProjectRegistry for Projects {
    fn initialize(&self) -> Result<(), RegistryError> {
        Ok(())
    }

    fn exists_on_disk(&self) -> bool {
        true
    }

    fn list(&self) -> Result<Vec<PersistedProjectRecord>, RegistryError> {
        Ok(self.0.lock().unwrap().clone())
    }

    fn get(&self, id: &str) -> Result<Option<PersistedProjectRecord>, RegistryError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .iter()
            .find(|project| project.project_id == id)
            .cloned())
    }

    fn get_or_create_active_by_root(
        &self,
        input: &ActiveProjectInput,
    ) -> Result<PersistedProjectRecord, RegistryError> {
        let mut records = self.0.lock().unwrap();
        if let Some(project) = records
            .iter_mut()
            .find(|project| project.root_path == input.root_path && project.archived_at.is_none())
        {
            project.kind = input.kind;
            project.project_key.clone_from(&input.project_key);
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
        let mut records = self.0.lock().unwrap();
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
        let mut records = self.0.lock().unwrap();
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
            .unwrap()
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

#[derive(Debug, Default)]
struct Workspaces(Mutex<Vec<PersistedWorkspaceRecord>>);

impl WorkspaceRegistry for Workspaces {
    fn initialize(&self) -> Result<(), RegistryError> {
        Ok(())
    }

    fn exists_on_disk(&self) -> bool {
        true
    }

    fn list(&self) -> Result<Vec<PersistedWorkspaceRecord>, RegistryError> {
        Ok(self.0.lock().unwrap().clone())
    }

    fn get(&self, id: &str) -> Result<Option<PersistedWorkspaceRecord>, RegistryError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .iter()
            .find(|workspace| workspace.workspace_id == id)
            .cloned())
    }

    fn upsert(
        &self,
        record: &PersistedWorkspaceRecord,
        _context: WorkspaceMutationContext,
    ) -> Result<(), RegistryError> {
        let mut records = self.0.lock().unwrap();
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
        let mut records = self.0.lock().unwrap();
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
        self.0
            .lock()
            .unwrap()
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

#[derive(Debug, Default)]
struct Source;

impl DirectorySource for Source {
    fn inspect(&self, path: &str) -> Result<Checkout, DirectorySourceError> {
        if path.contains("missing") {
            return Err(DirectorySourceError::NotFound);
        }
        let is_git = !path.contains("plain");
        Ok(Checkout {
            cwd: path.to_owned(),
            is_git,
            current_branch: is_git.then(|| "main".to_owned()),
            remote_url: is_git.then(|| "git@github.com:Example/Repo.git".to_owned()),
            worktree_root: is_git.then(|| "/tmp/alpha".to_owned()),
            is_paseo_owned_worktree: false,
            main_repo_root: None,
        })
    }

    fn create_child(&self, parent: &str, name: &str) -> Result<String, DirectorySourceError> {
        if name == "exists" {
            Err(DirectorySourceError::AlreadyExists)
        } else {
            Ok(format!("{parent}/{name}"))
        }
    }

    fn remove_empty(&self, _path: &str) -> Result<(), DirectorySourceError> {
        Ok(())
    }

    fn equivalent(&self, left: &str, right: &str) -> bool {
        left == right
    }

    fn canonical(&self, path: &str) -> Result<String, DirectorySourceError> {
        self.inspect(path).map(|checkout| checkout.cwd)
    }
}

#[derive(Debug, Default)]
struct ConfigStore(Mutex<Option<(serde_json::Value, StoreConfigRevision)>>);

impl ProjectConfigStore for ConfigStore {
    fn read(&self, _root: &str) -> Result<ProjectConfigDocument, ProjectConfigStoreError> {
        let value = self.0.lock().unwrap().clone();
        Ok(ProjectConfigDocument {
            config: value.as_ref().map(|(config, _)| config.clone()),
            revision: value.map(|(_, revision)| revision),
        })
    }

    fn write(
        &self,
        _root: &str,
        config: &serde_json::Value,
        expected_revision: Option<StoreConfigRevision>,
    ) -> Result<ProjectConfigWrite, ProjectConfigStoreError> {
        let mut value = self.0.lock().unwrap();
        let current = value.as_ref().map(|(_, revision)| *revision);
        if current != expected_revision {
            return Ok(ProjectConfigWrite::Stale {
                current_revision: current,
            });
        }
        let revision = StoreConfigRevision {
            mtime_ms: current.map_or(1.0, |revision| revision.mtime_ms + 1.0),
            size: 10.0,
        };
        *value = Some((config.clone(), revision));
        Ok(ProjectConfigWrite::Written {
            config: config.clone(),
            revision,
        })
    }
}

#[derive(Debug, Default)]
struct IconStore(Mutex<std::collections::BTreeMap<String, Vec<u8>>>);

impl ProjectIconStore for IconStore {
    fn write_custom(&self, project_id: &str, bytes: &[u8]) -> Result<(), ProjectIconStoreError> {
        if bytes.is_empty() {
            return Err(ProjectIconStoreError::Invalid);
        }
        self.0
            .lock()
            .unwrap()
            .insert(project_id.to_owned(), bytes.to_vec());
        Ok(())
    }

    fn remove_custom(&self, project_id: &str) -> Result<(), ProjectIconStoreError> {
        self.0.lock().unwrap().remove(project_id);
        Ok(())
    }

    fn read_custom(&self, project_id: &str) -> Result<Option<ProjectIcon>, ProjectIconStoreError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .get(project_id)
            .cloned()
            .map(|bytes| ProjectIcon {
                bytes,
                mime_type: "image/png".to_owned(),
            }))
    }

    fn find_automatic(
        &self,
        _project_root: &str,
    ) -> Result<Option<ProjectIcon>, ProjectIconStoreError> {
        Ok(None)
    }
}

fn project() -> PersistedProjectRecord {
    PersistedProjectRecord {
        project_id: "prj_a".to_owned(),
        root_path: "/tmp/alpha".to_owned(),
        kind: PersistedProjectKind::Git,
        display_name: "alpha".to_owned(),
        project_key: None,
        custom_name: None,
        custom_icon_revision: None,
        created_at: "created".to_owned(),
        updated_at: "created".to_owned(),
        archived_at: None,
    }
}

fn workspace() -> PersistedWorkspaceRecord {
    PersistedWorkspaceRecord {
        workspace_id: "wks_a".to_owned(),
        project_id: "prj_a".to_owned(),
        cwd: "/tmp/alpha".to_owned(),
        kind: PersistedWorkspaceKind::LocalCheckout,
        display_name: "main".to_owned(),
        title: None,
        branch: Some("main".to_owned()),
        worktree_root: Some("/tmp/alpha".to_owned()),
        base_branch: None,
        is_paseo_owned_worktree: false,
        main_repo_root: Some("/tmp/alpha".to_owned()),
        created_at: "created".to_owned(),
        updated_at: "created".to_owned(),
        archived_at: None,
        auto_archived_change_request_url: None,
        pinned_at: None,
        labels: None,
        untrusted_source: None,
    }
}

fn directory() -> Directory {
    Directory::new(DirectoryDependencies {
        projects: Box::new(Projects(Mutex::new(vec![project()]))),
        workspaces: Box::new(Workspaces(Mutex::new(vec![workspace()]))),
        source: Box::<Source>::default(),
        config_store: Box::<ConfigStore>::default(),
        icon_store: Box::<IconStore>::default(),
        server_id: "server-test".to_owned(),
    })
}

#[test]
fn coordinates_name_title_pin_and_archive_mutations() {
    let directory = directory();
    let renamed = directory
        .rename_project("prj_a", Some("Alpha"), "updated")
        .unwrap()
        .unwrap();
    assert_eq!(renamed.custom_name.as_deref(), Some("Alpha"));
    let titled = directory
        .set_workspace_title("wks_a", Some("Review"), "updated")
        .unwrap()
        .unwrap();
    assert_eq!(titled.title.as_deref(), Some("Review"));
    let pinned = directory
        .set_workspace_pin("wks_a", Some("pinned"), "updated")
        .unwrap()
        .unwrap();
    assert_eq!(pinned.pinned_at.as_deref(), Some("pinned"));
    let archived = directory
        .archive_workspace("wks_a", "archived")
        .unwrap()
        .unwrap();
    assert_eq!(archived.archived_at.as_deref(), Some("archived"));
}

#[test]
fn removal_archives_active_children_and_is_idempotent_for_missing_projects() {
    let directory = directory();
    assert_eq!(
        directory.remove_project("prj_a", "archived").unwrap(),
        ["wks_a"]
    );
    assert!(directory.list_projects().unwrap().is_empty());
    assert!(
        directory
            .remove_project("prj_a", "later")
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        directory.list_workspaces().unwrap()[0]
            .archived_at
            .as_deref(),
        Some("archived")
    );
}

#[test]
fn missing_mutation_targets_return_none() {
    let directory = directory();
    assert!(
        directory
            .rename_project("missing", None, "updated")
            .unwrap()
            .is_none()
    );
    assert!(
        directory
            .set_workspace_title("missing", None, "updated")
            .unwrap()
            .is_none()
    );
    assert!(
        directory
            .archive_workspace("missing", "updated")
            .unwrap()
            .is_none()
    );
}

#[test]
fn adds_selected_nested_roots_and_creates_fresh_workspaces() {
    let directory = directory();
    let project = directory.add_project("/tmp/alpha/nested", "one").unwrap();
    assert_eq!(project.root_path, "/tmp/alpha/nested");
    assert_eq!(
        project.project_key.as_deref(),
        Some("remote:github.com/example/repo#subdir:nested")
    );
    let first = directory
        .create_workspace(crate::service::directory::WorkspaceCreation {
            path: "/tmp/alpha/nested",
            title: Some("  First  ".to_owned()),
            project_id: Some(&project.project_id),
            workspace_id: Some("wks_first".to_owned()),
            expects_initial_agent: true,
            timestamp: "two",
        })
        .unwrap();
    let second = directory
        .create_workspace(crate::service::directory::WorkspaceCreation {
            path: "/tmp/alpha/nested",
            title: None,
            project_id: Some(&project.project_id),
            workspace_id: Some("wks_second".to_owned()),
            expects_initial_agent: false,
            timestamp: "three",
        })
        .unwrap();
    assert_eq!(first.title.as_deref(), Some("First"));
    assert_ne!(first.workspace_id, second.workspace_id);
}

#[test]
fn opening_reuses_active_and_restores_oldest_archived_workspace() {
    let directory = directory();
    assert_eq!(
        directory
            .open_workspace("/tmp/alpha", "later")
            .unwrap()
            .workspace_id,
        "wks_a"
    );
    directory.archive_workspace("wks_a", "archived").unwrap();
    let restored = directory.open_workspace("/tmp/alpha", "restored").unwrap();
    assert_eq!(restored.workspace_id, "wks_a");
    assert!(restored.archived_at.is_none());
}

#[test]
fn explicit_project_and_directory_creation_errors_match_paseo_classes() {
    let directory = directory();
    assert_eq!(
        directory.create_workspace(crate::service::directory::WorkspaceCreation {
            path: "/tmp/alpha",
            title: None,
            project_id: Some("missing"),
            workspace_id: None,
            expects_initial_agent: false,
            timestamp: "now"
        }),
        Err(DirectoryError::UnknownProject)
    );
    assert_eq!(
        directory.create_project_directory("/tmp", "../escape", "now"),
        Err(DirectoryError::InvalidDirectoryName)
    );
    assert_eq!(
        directory.create_project_directory("/tmp", "exists", "now"),
        Err(DirectoryError::DirectoryExists)
    );
    assert_eq!(
        directory.add_project("/tmp/missing", "now"),
        Err(DirectoryError::DirectoryNotFound)
    );
}

#[test]
fn project_key_parser_matches_paseo_remote_and_host_forms() {
    let mut checkout = Source.inspect("/tmp/alpha/nested").unwrap();
    assert_eq!(
        derive_project_key(&checkout, "server"),
        "remote:github.com/example/repo#subdir:nested"
    );
    checkout.remote_url = Some("ssh://git@git.example.com:60443/team/repo.git".to_owned());
    assert_eq!(
        derive_project_key(&checkout, "server"),
        "remote:git.example.com:60443/team/repo#subdir:nested"
    );
    checkout.remote_url = None;
    assert_eq!(
        derive_project_key(&checkout, "server"),
        "host:server:/tmp/alpha/nested"
    );
}

#[test]
fn project_config_requires_an_active_known_root_and_uses_revisions() {
    let directory = directory();
    let missing = directory.read_project_config("/tmp/alpha").unwrap();
    assert!(missing.config.is_none());
    let written = directory
        .write_project_config("/tmp/alpha", &serde_json::json!({"future":true}), None)
        .unwrap();
    assert_eq!(written.repo_root, "/tmp/alpha");
    assert_eq!(
        directory.read_project_config("/tmp/alpha").unwrap().config,
        Some(serde_json::json!({"future":true}))
    );
    assert!(matches!(
        directory.write_project_config("/tmp/alpha", &serde_json::json!({}), None),
        Err(DirectoryError::StaleProjectConfig {
            current_revision: Some(_)
        })
    ));
    assert_eq!(
        directory.read_project_config("/tmp/missing"),
        Err(DirectoryError::UnknownProject)
    );
}

#[test]
fn project_icon_round_trips_custom_bytes_and_returns_to_automatic() {
    let directory = directory();
    let updated = directory
        .set_project_icon("prj_a", Some(b"png"), "updated")
        .unwrap()
        .unwrap();
    assert!(updated.custom_icon_revision.is_some());
    let icon = directory.get_project_icon("prj_a").unwrap().unwrap();
    assert_eq!(icon.bytes, b"png");
    assert_eq!(icon.mime_type, "image/png");
    let automatic = directory
        .set_project_icon("prj_a", None, "later")
        .unwrap()
        .unwrap();
    assert!(automatic.custom_icon_revision.is_none());
    assert!(directory.get_project_icon("prj_a").unwrap().is_none());
    assert!(
        directory
            .set_project_icon("missing", Some(b"png"), "updated")
            .unwrap()
            .is_none()
    );
}
