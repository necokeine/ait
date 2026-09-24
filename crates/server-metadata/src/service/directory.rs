//! Project and workspace directory use cases over the Paseo-shaped registry ports.

use std::path::Path;
use std::sync::Arc;

use crate::model::registry::{
    PersistedProjectKind, PersistedProjectRecord, PersistedWorkspaceKind, PersistedWorkspaceRecord,
};
use crate::ports::provisioning::{
    Checkout, DirectorySource, DirectorySourceError, ProjectConfigRevision as StoreConfigRevision,
    ProjectConfigStore, ProjectConfigStoreError, ProjectConfigWrite, ProjectIconStore,
    ProjectIconStoreError,
};
use crate::ports::registry::{
    ActiveProjectInput, ProjectRegistry, RegistryError, WorkspaceArchiveContext,
    WorkspaceMutationContext, WorkspaceRegistry,
};

/// Safe application error for registry-backed directory operations.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum DirectoryError {
    /// Registry validation, storage, freeze, or observer handling failed.
    #[error("project or workspace registry operation failed")]
    Registry,
    /// The selected directory does not exist.
    #[error("directory not found")]
    DirectoryNotFound,
    /// A directory name violates the single-child-name contract.
    #[error("invalid directory name")]
    InvalidDirectoryName,
    /// The selected parent does not exist or is not a directory.
    #[error("parent directory not found")]
    ParentDirectoryNotFound,
    /// The target child directory already exists.
    #[error("directory already exists")]
    DirectoryExists,
    /// Filesystem permissions rejected the requested operation.
    #[error("permission denied")]
    PermissionDenied,
    /// A filesystem or Git inspection operation failed.
    #[error("filesystem operation failed")]
    FileSystem,
    /// Project registration failed after creating a directory.
    #[error("project registration failed")]
    RegistrationFailed {
        /// Directory that was created before registration failed.
        directory_path: String,
        /// Whether rollback also failed, so the directory can remain on disk.
        rollback_failed: bool,
    },
    /// An explicitly selected project does not exist.
    #[error("unknown project")]
    UnknownProject,
    /// An explicitly selected project is archived.
    #[error("archived project")]
    ArchivedProject,
    /// The saved project config is invalid JSON or violates its wire schema.
    #[error("invalid project config")]
    InvalidProjectConfig,
    /// The project config changed since the caller read it.
    #[error("stale project config")]
    StaleProjectConfig {
        /// Current revision, or none when the file was removed.
        current_revision: Option<ProjectConfigRevision>,
    },
    /// The project config could not be staged or atomically installed.
    #[error("project config write failed")]
    ProjectConfigWriteFailed,
    /// Uploaded icon bytes violate Paseo's size, format, or dimensions contract.
    #[error("invalid project icon")]
    InvalidProjectIcon,
    /// Custom icon persistence or automatic discovery failed.
    #[error("project icon storage failed")]
    ProjectIconStorage,
}

/// Application-level project configuration revision.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ProjectConfigRevision {
    /// Last modification time in Unix milliseconds.
    pub mtime_ms: f64,
    /// File size in bytes.
    pub size: f64,
}

/// Successful project configuration read.
#[derive(Debug, Clone, PartialEq)]
pub struct ProjectConfigRead {
    /// Canonical active project root.
    pub repo_root: String,
    /// Parsed document, or none when `paseo.json` is absent.
    pub config: Option<serde_json::Value>,
    /// Matching file revision, or none when absent.
    pub revision: Option<ProjectConfigRevision>,
}

/// Successful project configuration write.
#[derive(Debug, Clone, PartialEq)]
pub struct ProjectConfigWritten {
    /// Canonical active project root.
    pub repo_root: String,
    /// Installed document.
    pub config: serde_json::Value,
    /// Revision after installation.
    pub revision: ProjectConfigRevision,
}

/// Effective project icon bytes and MIME type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectIconValue {
    /// Raw validated image bytes.
    pub bytes: Vec<u8>,
    /// MIME type detected by the adapter.
    pub mime_type: String,
}

/// Parameters for creating a new Workspace registry record.
#[derive(Debug, Clone)]
pub struct WorkspaceCreation<'a> {
    /// Existing directory to inspect.
    pub path: &'a str,
    /// Optional user title.
    pub title: Option<String>,
    /// Explicit active owning Project, or automatic registration.
    pub project_id: Option<&'a str>,
    /// Caller-reserved identity, or a freshly generated identity.
    pub workspace_id: Option<String>,
    /// Whether a first Agent will follow creation.
    pub expects_initial_agent: bool,
    /// Creation and update timestamp.
    pub timestamp: &'a str,
}

/// Blocking project/workspace coordinator; clones share the same registries and adapters.
#[derive(Debug, Clone)]
pub struct Directory {
    projects: Arc<dyn ProjectRegistry>,
    workspaces: Arc<dyn WorkspaceRegistry>,
    source: Arc<dyn DirectorySource>,
    config_store: Arc<dyn ProjectConfigStore>,
    icon_store: Arc<dyn ProjectIconStore>,
    server_id: String,
}

/// Independent adapters composed for Project and Workspace directory use cases.
#[derive(Debug)]
pub struct DirectoryDependencies {
    /// Durable Project records.
    pub projects: Box<dyn ProjectRegistry>,
    /// Durable Workspace records.
    pub workspaces: Box<dyn WorkspaceRegistry>,
    /// Local directory inspection and creation.
    pub source: Box<dyn DirectorySource>,
    /// `paseo.json` persistence.
    pub config_store: Box<dyn ProjectConfigStore>,
    /// Project icon persistence.
    pub icon_store: Box<dyn ProjectIconStore>,
    /// Stable current server identity.
    pub server_id: String,
}

impl Directory {
    /// Compose independent registry adapters.
    #[must_use]
    pub fn new(dependencies: DirectoryDependencies) -> Self {
        Self {
            projects: dependencies.projects.into(),
            workspaces: dependencies.workspaces.into(),
            source: dependencies.source.into(),
            config_store: dependencies.config_store.into(),
            icon_store: dependencies.icon_store.into(),
            server_id: dependencies.server_id,
        }
    }

    /// Register or refresh the oldest active project for a selected directory.
    ///
    /// The selected directory remains the project root even when it is nested below a Git root.
    ///
    /// # Errors
    /// Returns a categorized inspection or registry error.
    pub fn add_project(
        &self,
        path: &str,
        timestamp: &str,
    ) -> Result<PersistedProjectRecord, DirectoryError> {
        let checkout = self.source.inspect(path).map_err(map_source)?;
        self.project_for_checkout(&checkout, timestamp)
    }

    /// Atomically create one child directory and register it as a project where possible.
    ///
    /// # Errors
    /// Returns validation, filesystem, rollback, or registry errors with Paseo-compatible classes.
    pub fn create_project_directory(
        &self,
        parent_path: &str,
        name: &str,
        timestamp: &str,
    ) -> Result<(String, PersistedProjectRecord), DirectoryError> {
        validate_directory_name(name)?;
        if parent_path.trim().is_empty() {
            return Err(DirectoryError::ParentDirectoryNotFound);
        }
        let parent = self
            .source
            .inspect(parent_path.trim())
            .map_err(map_parent_source)?;
        let directory_path = self
            .source
            .create_child(&parent.cwd, name)
            .map_err(map_create_source)?;
        if let Ok(project) = self.add_project(&directory_path, timestamp) {
            Ok((directory_path, project))
        } else {
            let rollback_failed = self.source.remove_empty(&directory_path).is_err();
            Err(DirectoryError::RegistrationFailed {
                directory_path,
                rollback_failed,
            })
        }
    }

    /// Always create a fresh workspace record for an existing directory.
    ///
    /// # Errors
    /// Returns a categorized inspection, explicit-project, identity, or registry error.
    pub fn create_workspace(
        &self,
        creation: WorkspaceCreation<'_>,
    ) -> Result<PersistedWorkspaceRecord, DirectoryError> {
        let WorkspaceCreation {
            path,
            title,
            project_id,
            workspace_id,
            expects_initial_agent,
            timestamp,
        } = creation;
        let checkout = self.source.inspect(path).map_err(map_source)?;
        let project = match project_id {
            Some(project_id) => self.require_active_project(project_id, &checkout, timestamp)?,
            None => self.project_for_checkout(&checkout, timestamp)?,
        };
        let workspace = workspace_record(
            workspace_id.unwrap_or(generate_workspace_id()?),
            &project.project_id,
            &checkout,
            normalize_optional_text(title),
            timestamp,
        );
        self.workspaces
            .upsert(
                &workspace,
                WorkspaceMutationContext {
                    expects_initial_agent: expects_initial_agent.then_some(true),
                },
            )
            .map_err(map_error)?;
        Ok(workspace)
    }

    /// Reuse, restore, or create the oldest workspace for an existing directory.
    ///
    /// # Errors
    /// Returns a categorized inspection or registry error.
    pub fn open_workspace(
        &self,
        path: &str,
        timestamp: &str,
    ) -> Result<PersistedWorkspaceRecord, DirectoryError> {
        let checkout = self.source.inspect(path).map_err(map_source)?;
        let mut matching = self
            .workspaces
            .list()
            .map_err(map_error)?
            .into_iter()
            .filter(|workspace| self.source.equivalent(&workspace.cwd, &checkout.cwd))
            .collect::<Vec<_>>();
        matching.sort_by(|left, right| {
            left.created_at
                .cmp(&right.created_at)
                .then_with(|| left.workspace_id.cmp(&right.workspace_id))
        });
        if let Some(active) = matching
            .iter()
            .find(|workspace| workspace.archived_at.as_ref().is_none_or(String::is_empty))
        {
            return self.refresh_workspace(active, &checkout, timestamp);
        }
        if let Some(archived) = matching.iter().find(|workspace| {
            workspace
                .archived_at
                .as_ref()
                .is_some_and(|value| !value.is_empty())
        }) && self
            .projects
            .get(&archived.project_id)
            .map_err(map_error)?
            .is_some_and(|project| project.archived_at.as_ref().is_none_or(String::is_empty))
        {
            let mut restored = placement(archived.clone(), &checkout, timestamp);
            restored.archived_at = None;
            self.workspaces
                .upsert(&restored, WorkspaceMutationContext::default())
                .map_err(map_error)?;
            self.refresh_project_for_workspace(&restored, &checkout, timestamp)?;
            return Ok(restored);
        }
        self.create_workspace(crate::service::directory::WorkspaceCreation {
            path: &checkout.cwd,
            title: None,
            project_id: None,
            workspace_id: None,
            expects_initial_agent: false,
            timestamp,
        })
    }

    /// List all project records in registry insertion order.
    ///
    /// # Errors
    /// Returns `DirectoryError::Registry` when the registry cannot be read.
    pub fn list_projects(&self) -> Result<Vec<PersistedProjectRecord>, DirectoryError> {
        self.projects.list().map_err(map_error)
    }

    /// List all workspace records in registry insertion order.
    ///
    /// # Errors
    /// Returns `DirectoryError::Registry` when the registry cannot be read.
    pub fn list_workspaces(&self) -> Result<Vec<PersistedWorkspaceRecord>, DirectoryError> {
        self.workspaces.list().map_err(map_error)
    }

    /// Read `paseo.json` for a known active project root.
    ///
    /// # Errors
    /// Returns project-not-found or invalid-config errors inline to the API layer.
    pub fn read_project_config(
        &self,
        requested_root: &str,
    ) -> Result<ProjectConfigRead, DirectoryError> {
        let repo_root = self.resolve_active_project_root(requested_root)?;
        let document = self
            .config_store
            .read(&repo_root)
            .map_err(map_config_read_error)?;
        Ok(ProjectConfigRead {
            repo_root,
            config: document.config,
            revision: document.revision.map(application_config_revision),
        })
    }

    /// Optimistically write `paseo.json` for a known active project root.
    ///
    /// # Errors
    /// Returns project-not-found, stale-revision, or write failures inline to the API layer.
    pub fn write_project_config(
        &self,
        requested_root: &str,
        config: &serde_json::Value,
        expected_revision: Option<ProjectConfigRevision>,
    ) -> Result<ProjectConfigWritten, DirectoryError> {
        let repo_root = self.resolve_active_project_root(requested_root)?;
        match self
            .config_store
            .write(
                &repo_root,
                config,
                expected_revision.map(store_config_revision),
            )
            .map_err(map_config_write_error)?
        {
            ProjectConfigWrite::Written { config, revision } => Ok(ProjectConfigWritten {
                repo_root,
                config,
                revision: application_config_revision(revision),
            }),
            ProjectConfigWrite::Stale { current_revision } => {
                Err(DirectoryError::StaleProjectConfig {
                    current_revision: current_revision.map(application_config_revision),
                })
            }
        }
    }

    /// Store uploaded custom icon bytes or return a project to automatic discovery.
    ///
    /// # Errors
    /// Returns missing-project, invalid-image, registry, or icon-storage failures.
    pub fn set_project_icon(
        &self,
        project_id: &str,
        upload: Option<&[u8]>,
        timestamp: &str,
    ) -> Result<Option<PersistedProjectRecord>, DirectoryError> {
        if self.projects.get(project_id).map_err(map_error)?.is_none() {
            return Ok(None);
        }
        let revision = if let Some(bytes) = upload {
            self.icon_store
                .write_custom(project_id, bytes)
                .map_err(map_icon_error)?;
            Some(generate_icon_revision()?)
        } else {
            self.icon_store
                .remove_custom(project_id)
                .map_err(map_icon_error)?;
            None
        };
        let updated = self
            .projects
            .update(project_id, &|project| {
                let mut project = project.clone();
                project.custom_icon_revision.clone_from(&revision);
                timestamp.clone_into(&mut project.updated_at);
                project
            })
            .map_err(map_error)?;
        if updated.is_none() {
            let _ = self.icon_store.remove_custom(project_id);
        }
        Ok(updated)
    }

    /// Read the effective custom or automatically discovered icon.
    ///
    /// # Errors
    /// Returns missing-project or icon-storage failures.
    pub fn get_project_icon(
        &self,
        project_id: &str,
    ) -> Result<Option<ProjectIconValue>, DirectoryError> {
        let Some(project) = self.projects.get(project_id).map_err(map_error)? else {
            return Err(DirectoryError::UnknownProject);
        };
        let icon = if project.custom_icon_revision.is_some() {
            self.icon_store
                .read_custom(project_id)
                .map_err(map_icon_error)?
        } else {
            self.icon_store
                .find_automatic(&project.root_path)
                .map_err(map_icon_error)?
        };
        Ok(icon.map(|icon| ProjectIconValue {
            bytes: icon.bytes,
            mime_type: icon.mime_type,
        }))
    }

    /// Update a project name, returning none when the identity is absent.
    ///
    /// # Errors
    /// Returns `DirectoryError::Registry` when validation or persistence fails.
    pub fn rename_project(
        &self,
        project_id: &str,
        custom_name: Option<&str>,
        updated_at: &str,
    ) -> Result<Option<PersistedProjectRecord>, DirectoryError> {
        self.projects
            .update(project_id, &|project| {
                let mut project = project.clone();
                project.custom_name = custom_name.map(str::to_owned);
                updated_at.clone_into(&mut project.updated_at);
                project
            })
            .map_err(map_error)
    }

    /// Archive active child workspaces and remove a project record.
    ///
    /// Returns the workspace identities archived by this call. As in Paseo, an absent project is
    /// an idempotent successful removal.
    ///
    /// # Errors
    /// Returns `DirectoryError::Registry` after any committed prefix if a later write fails.
    pub fn remove_project(
        &self,
        project_id: &str,
        archived_at: &str,
    ) -> Result<Vec<String>, DirectoryError> {
        let active_workspace_ids = self
            .workspaces
            .list()
            .map_err(map_error)?
            .into_iter()
            .filter(|workspace| {
                workspace.project_id == project_id
                    && workspace.archived_at.as_ref().is_none_or(String::is_empty)
            })
            .map(|workspace| workspace.workspace_id)
            .collect::<Vec<_>>();
        for workspace_id in &active_workspace_ids {
            self.workspaces
                .archive(
                    workspace_id,
                    archived_at,
                    &WorkspaceArchiveContext::default(),
                )
                .map_err(map_error)?;
        }
        self.projects.remove(project_id).map_err(map_error)?;
        let _ = self.icon_store.remove_custom(project_id);
        Ok(active_workspace_ids)
    }

    /// Archive a workspace record, returning none when it is absent.
    ///
    /// # Errors
    /// Returns `DirectoryError::Registry` when the registry cannot read or persist the record.
    pub fn archive_workspace(
        &self,
        workspace_id: &str,
        archived_at: &str,
    ) -> Result<Option<PersistedWorkspaceRecord>, DirectoryError> {
        if self
            .workspaces
            .get(workspace_id)
            .map_err(map_error)?
            .is_none()
        {
            return Ok(None);
        }
        self.workspaces
            .archive(
                workspace_id,
                archived_at,
                &WorkspaceArchiveContext::default(),
            )
            .map_err(map_error)?;
        self.workspaces.get(workspace_id).map_err(map_error)
    }

    /// Set a workspace title, returning none when it is absent.
    ///
    /// # Errors
    /// Returns `DirectoryError::Registry` when validation or persistence fails.
    pub fn set_workspace_title(
        &self,
        workspace_id: &str,
        title: Option<&str>,
        updated_at: &str,
    ) -> Result<Option<PersistedWorkspaceRecord>, DirectoryError> {
        self.workspaces
            .update(workspace_id, &|workspace| {
                let mut workspace = workspace.clone();
                workspace.title = title.map(str::to_owned);
                updated_at.clone_into(&mut workspace.updated_at);
                workspace
            })
            .map_err(map_error)
    }

    /// Set a workspace pin timestamp, returning none when it is absent.
    ///
    /// # Errors
    /// Returns `DirectoryError::Registry` when validation or persistence fails.
    pub fn set_workspace_pin(
        &self,
        workspace_id: &str,
        pinned_at: Option<&str>,
        updated_at: &str,
    ) -> Result<Option<PersistedWorkspaceRecord>, DirectoryError> {
        self.workspaces
            .update(workspace_id, &|workspace| {
                let mut workspace = workspace.clone();
                workspace.pinned_at = pinned_at.map(str::to_owned);
                updated_at.clone_into(&mut workspace.updated_at);
                workspace
            })
            .map_err(map_error)
    }

    fn project_for_checkout(
        &self,
        checkout: &Checkout,
        timestamp: &str,
    ) -> Result<PersistedProjectRecord, DirectoryError> {
        self.projects
            .get_or_create_active_by_root(&ActiveProjectInput {
                root_path: checkout.cwd.clone(),
                kind: project_kind(checkout),
                display_name: basename(&checkout.cwd),
                project_key: Some(derive_project_key(checkout, &self.server_id)),
                timestamp: timestamp.to_owned(),
            })
            .map_err(map_error)
    }

    fn require_active_project(
        &self,
        project_id: &str,
        workspace_checkout: &Checkout,
        timestamp: &str,
    ) -> Result<PersistedProjectRecord, DirectoryError> {
        let project = self
            .projects
            .get(project_id)
            .map_err(map_error)?
            .ok_or(DirectoryError::UnknownProject)?;
        if project
            .archived_at
            .as_ref()
            .is_some_and(|value| !value.is_empty())
        {
            return Err(DirectoryError::ArchivedProject);
        }
        let checkout = if self
            .source
            .equivalent(&project.root_path, &workspace_checkout.cwd)
        {
            workspace_checkout.clone()
        } else {
            self.source
                .inspect(&project.root_path)
                .map_err(map_source)?
        };
        self.refresh_project(project, &checkout, timestamp)
    }

    fn refresh_project_for_workspace(
        &self,
        workspace: &PersistedWorkspaceRecord,
        workspace_checkout: &Checkout,
        timestamp: &str,
    ) -> Result<(), DirectoryError> {
        if let Some(project) = self
            .projects
            .get(&workspace.project_id)
            .map_err(map_error)?
        {
            let checkout = if self.source.equivalent(&project.root_path, &workspace.cwd) {
                workspace_checkout.clone()
            } else {
                self.source
                    .inspect(&project.root_path)
                    .map_err(map_source)?
            };
            self.refresh_project(project, &checkout, timestamp)?;
        }
        Ok(())
    }

    fn refresh_project(
        &self,
        project: PersistedProjectRecord,
        checkout: &Checkout,
        timestamp: &str,
    ) -> Result<PersistedProjectRecord, DirectoryError> {
        let kind = project_kind(checkout);
        let key = Some(derive_project_key(checkout, &self.server_id));
        if project.kind == kind && project.project_key == key {
            return Ok(project);
        }
        let mut updated = project;
        updated.kind = kind;
        updated.project_key = key;
        timestamp.clone_into(&mut updated.updated_at);
        self.projects.upsert(&updated).map_err(map_error)?;
        Ok(updated)
    }

    fn refresh_workspace(
        &self,
        workspace: &PersistedWorkspaceRecord,
        checkout: &Checkout,
        timestamp: &str,
    ) -> Result<PersistedWorkspaceRecord, DirectoryError> {
        self.refresh_project_for_workspace(workspace, checkout, timestamp)?;
        let updated = placement(workspace.clone(), checkout, timestamp);
        if updated == *workspace {
            return Ok(updated);
        }
        self.workspaces
            .upsert(&updated, WorkspaceMutationContext::default())
            .map_err(map_error)?;
        Ok(updated)
    }

    fn resolve_active_project_root(&self, requested_root: &str) -> Result<String, DirectoryError> {
        let requested = self
            .source
            .canonical(requested_root)
            .map_err(|_| DirectoryError::UnknownProject)?;
        for project in self.projects.list().map_err(map_error)? {
            if project
                .archived_at
                .as_ref()
                .is_some_and(|value| !value.is_empty())
            {
                continue;
            }
            let Ok(root) = self.source.canonical(&project.root_path) else {
                continue;
            };
            if self.source.equivalent(&root, &requested) {
                return Ok(root);
            }
        }
        Err(DirectoryError::UnknownProject)
    }
}

const fn map_error(_error: RegistryError) -> DirectoryError {
    DirectoryError::Registry
}

const fn map_source(error: DirectorySourceError) -> DirectoryError {
    match error {
        DirectorySourceError::NotFound => DirectoryError::DirectoryNotFound,
        DirectorySourceError::AlreadyExists => DirectoryError::DirectoryExists,
        DirectorySourceError::PermissionDenied => DirectoryError::PermissionDenied,
        DirectorySourceError::Io => DirectoryError::FileSystem,
    }
}

const fn map_parent_source(error: DirectorySourceError) -> DirectoryError {
    match error {
        DirectorySourceError::NotFound | DirectorySourceError::AlreadyExists => {
            DirectoryError::ParentDirectoryNotFound
        }
        DirectorySourceError::PermissionDenied => DirectoryError::PermissionDenied,
        DirectorySourceError::Io => DirectoryError::FileSystem,
    }
}

const fn map_create_source(error: DirectorySourceError) -> DirectoryError {
    match error {
        DirectorySourceError::NotFound => DirectoryError::ParentDirectoryNotFound,
        DirectorySourceError::AlreadyExists => DirectoryError::DirectoryExists,
        DirectorySourceError::PermissionDenied => DirectoryError::PermissionDenied,
        DirectorySourceError::Io => DirectoryError::FileSystem,
    }
}

const fn map_config_read_error(_error: ProjectConfigStoreError) -> DirectoryError {
    DirectoryError::InvalidProjectConfig
}

const fn map_config_write_error(_error: ProjectConfigStoreError) -> DirectoryError {
    DirectoryError::ProjectConfigWriteFailed
}

const fn map_icon_error(error: ProjectIconStoreError) -> DirectoryError {
    match error {
        ProjectIconStoreError::Invalid => DirectoryError::InvalidProjectIcon,
        ProjectIconStoreError::Io => DirectoryError::ProjectIconStorage,
    }
}

const fn application_config_revision(revision: StoreConfigRevision) -> ProjectConfigRevision {
    ProjectConfigRevision {
        mtime_ms: revision.mtime_ms,
        size: revision.size,
    }
}

const fn store_config_revision(revision: ProjectConfigRevision) -> StoreConfigRevision {
    StoreConfigRevision {
        mtime_ms: revision.mtime_ms,
        size: revision.size,
    }
}

fn validate_directory_name(name: &str) -> Result<(), DirectoryError> {
    if name.trim().is_empty()
        || name != name.trim()
        || matches!(name, "." | "..")
        || name.contains(['/', '\\', '\0'])
        || name.as_bytes().get(1) == Some(&b':')
        || Path::new(name).is_absolute()
    {
        return Err(DirectoryError::InvalidDirectoryName);
    }
    Ok(())
}

const fn project_kind(checkout: &Checkout) -> PersistedProjectKind {
    if checkout.is_git {
        PersistedProjectKind::Git
    } else {
        PersistedProjectKind::NonGit
    }
}

fn workspace_record(
    workspace_id: String,
    project_id: &str,
    checkout: &Checkout,
    title: Option<String>,
    timestamp: &str,
) -> PersistedWorkspaceRecord {
    PersistedWorkspaceRecord {
        workspace_id,
        project_id: project_id.to_owned(),
        cwd: checkout.cwd.clone(),
        kind: workspace_kind(checkout),
        display_name: checkout
            .current_branch
            .as_deref()
            .filter(|branch| !branch.trim().is_empty() && !branch.eq_ignore_ascii_case("HEAD"))
            .map_or_else(|| basename(&checkout.cwd), str::to_owned),
        title,
        branch: normalize_branch(checkout.current_branch.clone()),
        worktree_root: checkout.is_git.then(|| {
            checkout
                .worktree_root
                .clone()
                .unwrap_or_else(|| checkout.cwd.clone())
        }),
        base_branch: None,
        is_paseo_owned_worktree: checkout.is_git && checkout.is_paseo_owned_worktree,
        main_repo_root: checkout
            .is_git
            .then(|| checkout.main_repo_root.clone())
            .flatten(),
        created_at: timestamp.to_owned(),
        updated_at: timestamp.to_owned(),
        archived_at: None,
        auto_archived_change_request_url: None,
        pinned_at: None,
        labels: None,
        untrusted_source: None,
    }
}

fn placement(
    mut workspace: PersistedWorkspaceRecord,
    checkout: &Checkout,
    timestamp: &str,
) -> PersistedWorkspaceRecord {
    let kind = workspace_kind(checkout);
    let branch = normalize_branch(checkout.current_branch.clone());
    let worktree_root = checkout.is_git.then(|| {
        checkout
            .worktree_root
            .clone()
            .unwrap_or_else(|| checkout.cwd.clone())
    });
    let main_repo_root = checkout
        .is_git
        .then(|| checkout.main_repo_root.clone())
        .flatten();
    let owned = checkout.is_git && checkout.is_paseo_owned_worktree;
    if workspace.kind != kind
        || workspace.branch != branch
        || workspace.worktree_root != worktree_root
        || workspace.main_repo_root != main_repo_root
        || workspace.is_paseo_owned_worktree != owned
    {
        workspace.kind = kind;
        workspace.branch = branch;
        workspace.worktree_root = worktree_root;
        workspace.main_repo_root = main_repo_root;
        workspace.is_paseo_owned_worktree = owned;
        timestamp.clone_into(&mut workspace.updated_at);
    }
    workspace
}

const fn workspace_kind(checkout: &Checkout) -> PersistedWorkspaceKind {
    if !checkout.is_git {
        PersistedWorkspaceKind::Directory
    } else if checkout.main_repo_root.is_some() {
        PersistedWorkspaceKind::Worktree
    } else {
        PersistedWorkspaceKind::LocalCheckout
    }
}

fn normalize_branch(branch: Option<String>) -> Option<String> {
    branch
        .map(|branch| branch.trim().to_owned())
        .filter(|branch| !branch.is_empty() && !branch.eq_ignore_ascii_case("HEAD"))
}

fn normalize_optional_text(text: Option<String>) -> Option<String> {
    text.map(|text| text.trim().to_owned())
        .filter(|text| !text.is_empty())
}

/// Return the final UTF-8 path component, falling back to the supplied path.
#[must_use]
pub fn basename(path: &str) -> String {
    Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or(path)
        .to_owned()
}

/// Allocate a fresh Paseo Workspace identity using operating-system randomness.
///
/// # Errors
/// Returns a filesystem error if the random source is unavailable.
pub fn generate_workspace_id() -> Result<String, DirectoryError> {
    let mut bytes = [0_u8; 8];
    getrandom::fill(&mut bytes).map_err(|_| DirectoryError::FileSystem)?;
    Ok(format!("wks_{}", hex(&bytes)))
}

fn generate_icon_revision() -> Result<String, DirectoryError> {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes).map_err(|_| DirectoryError::ProjectIconStorage)?;
    let encoded = hex(&bytes);
    Ok(format!(
        "{}-{}-{}-{}-{}",
        &encoded[0..8],
        &encoded[8..12],
        &encoded[12..16],
        &encoded[16..20],
        &encoded[20..32]
    ))
}

fn hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut result = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        result.push(char::from(HEX[usize::from(byte >> 4)]));
        result.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    result
}

/// Derive a grouping key from a checkout's remote or host-local directory identity.
#[must_use]
pub fn derive_project_key(checkout: &Checkout, server_id: &str) -> String {
    let selected_path = checkout
        .worktree_root
        .as_deref()
        .and_then(|root| Path::new(&checkout.cwd).strip_prefix(root).ok())
        .filter(|path| !path.as_os_str().is_empty())
        .and_then(Path::to_str)
        .map(|path| path.replace('\\', "/"));
    if let Some(remote) = checkout.remote_url.as_deref().and_then(parse_remote) {
        let mut path = remote.path;
        if remote.host == "github.com" {
            path.make_ascii_lowercase();
        }
        let host = remote.port.map_or_else(
            || remote.host.clone(),
            |port| format!("{}:{port}", remote.host),
        );
        let key = format!("remote:{host}/{path}");
        return selected_path.map_or(key.clone(), |path| format!("{key}#subdir:{path}"));
    }
    let root = match (&selected_path, checkout.main_repo_root.as_deref()) {
        (Some(selected), Some(main)) => Path::new(main).join(selected),
        _ => Path::new(&checkout.cwd).to_path_buf(),
    };
    format!(
        "host:{server_id}:{}",
        root.to_string_lossy().replace('\\', "/")
    )
}

/// Parsed remote identity used by Project identity and repository provisioning.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteLocation {
    /// Normalized host.
    pub host: String,
    /// Nondefault port, if supplied.
    pub port: Option<String>,
    /// Decoded repository path without a trailing `.git`.
    pub path: String,
}

/// Parse a supported Git remote URL into a stable identity, or return none.
#[must_use]
pub fn parse_remote(remote: &str) -> Option<RemoteLocation> {
    let remote = remote.trim();
    if !remote.contains("://")
        && let Some((authority, path)) = remote.split_once(':')
        && let Some((_, host)) = authority.rsplit_once('@')
    {
        return remote_location(host, None, path);
    }
    let (scheme, remainder) = remote.split_once("://")?;
    let default_port = match scheme.to_ascii_lowercase().as_str() {
        "http" => "80",
        "https" => "443",
        "ssh" => "22",
        _ => return None,
    };
    let (authority, path) = remainder.split_once('/')?;
    let host_and_port = authority
        .rsplit_once('@')
        .map_or(authority, |(_, value)| value);
    let (host, port) = match host_and_port.rsplit_once(':') {
        Some((host, port))
            if !port.is_empty() && port.bytes().all(|byte| byte.is_ascii_digit()) =>
        {
            (host, (port != default_port).then(|| port.to_owned()))
        }
        _ => (host_and_port, None),
    };
    let path = path.split(['?', '#']).next()?;
    remote_location(host, port, &percent_decode(path)?)
}

fn remote_location(host: &str, port: Option<String>, path: &str) -> Option<RemoteLocation> {
    let host = host.trim().trim_end_matches('.').to_ascii_lowercase();
    let path = path
        .trim()
        .trim_matches('/')
        .strip_suffix(".git")
        .unwrap_or_else(|| path.trim().trim_matches('/'))
        .to_owned();
    let valid_host = host
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        && host
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
        && host
            .as_bytes()
            .last()
            .is_some_and(u8::is_ascii_alphanumeric);
    (valid_host && !path.is_empty()).then_some(RemoteLocation { host, port, path })
}

fn percent_decode(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let high = hex_digit(*bytes.get(index + 1)?)?;
            let low = hex_digit(*bytes.get(index + 2)?)?;
            decoded.push((high << 4) | low);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(decoded).ok()
}

const fn hex_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests;
