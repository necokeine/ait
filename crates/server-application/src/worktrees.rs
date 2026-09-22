//! Worktree lifecycle coordination over Git and Paseo-shaped registry ports.

use server_domain::registry::{
    PersistedProjectKind, PersistedProjectRecord, PersistedWorkspaceKind, PersistedWorkspaceRecord,
};
use server_ports::provisioning::Checkout;
use server_ports::registry::{
    ActiveProjectInput, ProjectRegistry, RegistryError, WorkspaceArchiveContext,
    WorkspaceMutationContext, WorkspaceRegistry,
};
use server_ports::worktrees::{
    CreatedManagedWorktree, ManagedWorktreeCreate, ManagedWorktreeInfo, ManagedWorktrees,
    OwnedWorktree, WorktreeCreateMode, WorktreeError,
};

use crate::directory::{basename, derive_project_key, generate_workspace_id};

/// Create action independent of the transport schema.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CreateAction {
    /// Create a new branch from the selected or default base.
    BranchOff,
    /// Check out an existing branch.
    Checkout,
}

/// Validated worktree creation intent from the API layer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateWorktree {
    /// Selected source directory.
    pub cwd: String,
    /// Optional owning project identity.
    pub project_id: Option<String>,
    /// Optional worktree name seed.
    pub worktree_slug: Option<String>,
    /// Optional base or checkout ref.
    pub ref_name: Option<String>,
    /// Explicit or default action.
    pub action: CreateAction,
    /// Whether a forge change-request source was supplied.
    pub has_change_request_source: bool,
    /// First-Agent prompt used as a provisional workspace title.
    pub first_agent_prompt: Option<String>,
    /// Whether the caller supplied any first-Agent context.
    pub expects_initial_agent: bool,
}

/// Registered worktree creation result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreatedWorkspace {
    /// Persisted workspace.
    pub workspace: PersistedWorkspaceRecord,
    /// Owning project used for descriptor construction.
    pub project: PersistedProjectRecord,
}

/// Archive scope independent of the transport schema.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchiveScope {
    /// Archive one workspace record.
    Workspace,
    /// Archive all workspace records in one managed worktree.
    Worktree,
}

/// Worktree archive selection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveWorktree {
    /// Exact target path, when supplied.
    pub worktree_path: Option<String>,
    /// Main repository root.
    pub repo_root: Option<String>,
    /// Managed worktree directory name.
    pub worktree_slug: Option<String>,
    /// Branch used to locate a managed worktree.
    pub branch_name: Option<String>,
    /// Exact workspace record to archive.
    pub workspace_id: Option<String>,
    /// Archive granularity.
    pub scope: ArchiveScope,
}

/// Completed archive effects.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchivedWorktree {
    /// Workspace identities archived by this operation.
    pub workspace_ids: Vec<String>,
}

/// Worktree lifecycle failure.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum WorktreesError {
    /// Git or filesystem adapter failure.
    #[error(transparent)]
    Worktree(#[from] WorktreeError),
    /// Registry persistence or validation failed.
    #[error("project or workspace registry operation failed")]
    Registry,
    /// An explicitly selected project does not exist.
    #[error("Unknown project: {0}")]
    UnknownProject(String),
    /// An explicitly selected project is archived.
    #[error("Archived project: {0}")]
    ArchivedProject(String),
    /// Creation failed after Git succeeded and rollback also failed.
    #[error("{cause}; rollback also failed: {rollback}")]
    Rollback {
        /// Original registry or provisioning failure.
        cause: Box<Self>,
        /// Cleanup failure.
        rollback: WorktreeError,
    },
}

/// Public classification used by transports without depending on adapter contracts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorktreeFailureKind {
    /// The selected directory is not in a Git repository.
    NotGitRepository,
    /// The archive target is outside the managed root.
    NotAllowed,
    /// A branch is already checked out.
    BranchAlreadyCheckedOut,
    /// Checkout omitted a branch or change request.
    MissingCheckoutTarget,
    /// Checkout named an unknown branch.
    UnknownBranch,
    /// Any other Git, registry, validation, or rollback failure.
    Other,
}

impl WorktreesError {
    /// Return the stable transport-facing failure class.
    #[must_use]
    pub fn kind(&self) -> WorktreeFailureKind {
        match self {
            Self::Worktree(error) => match error {
                WorktreeError::NotGitRepository => WorktreeFailureKind::NotGitRepository,
                WorktreeError::NotAllowed => WorktreeFailureKind::NotAllowed,
                WorktreeError::BranchAlreadyCheckedOut(_) => {
                    WorktreeFailureKind::BranchAlreadyCheckedOut
                }
                WorktreeError::MissingCheckoutTarget => WorktreeFailureKind::MissingCheckoutTarget,
                WorktreeError::UnknownBranch(_) => WorktreeFailureKind::UnknownBranch,
                WorktreeError::Invalid(_)
                | WorktreeError::ForgeUnavailable
                | WorktreeError::Io(_) => WorktreeFailureKind::Other,
            },
            Self::Rollback { cause, .. } => cause.kind(),
            Self::Registry | Self::UnknownProject(_) | Self::ArchivedProject(_) => {
                WorktreeFailureKind::Other
            }
        }
    }
}

/// Serialized worktree application service.
#[derive(Debug)]
pub struct Worktrees {
    projects: Box<dyn ProjectRegistry>,
    workspaces: Box<dyn WorkspaceRegistry>,
    managed: Box<dyn ManagedWorktrees>,
    server_id: String,
}

impl Worktrees {
    /// Compose independent registry and local Git adapters.
    #[must_use]
    pub fn new(
        projects: Box<dyn ProjectRegistry>,
        workspaces: Box<dyn WorkspaceRegistry>,
        managed: Box<dyn ManagedWorktrees>,
        server_id: String,
    ) -> Self {
        Self {
            projects,
            workspaces,
            managed,
            server_id,
        }
    }

    /// List server-owned worktrees for a repository.
    ///
    /// # Errors
    /// Returns categorized Git and filesystem failures.
    pub fn list(&self, cwd: &str) -> Result<Vec<ManagedWorktreeInfo>, WorktreesError> {
        self.managed.list(cwd).map_err(Into::into)
    }

    /// Create a Git worktree and then register its workspace, rolling back Git on later failure.
    ///
    /// # Errors
    /// Returns validation, Git, project selection, registry, or rollback errors.
    pub fn create(
        &self,
        input: &CreateWorktree,
        timestamp: &str,
    ) -> Result<CreatedWorkspace, WorktreesError> {
        if input.has_change_request_source {
            return Err(WorktreeError::ForgeUnavailable.into());
        }
        let slug = input
            .worktree_slug
            .as_deref()
            .map(slugify)
            .filter(|slug| !slug.is_empty())
            .map_or_else(random_slug, Ok)?;
        let mode = match input.action {
            CreateAction::BranchOff => WorktreeCreateMode::BranchOff {
                base_ref: normalize_ref(input.ref_name.as_deref()).map(str::to_owned),
                branch_name: slug.clone(),
            },
            CreateAction::Checkout => WorktreeCreateMode::Checkout {
                branch_name: normalize_ref(input.ref_name.as_deref())
                    .map(str::to_owned)
                    .ok_or(WorktreeError::MissingCheckoutTarget)?,
            },
        };
        let created = self.managed.create(&ManagedWorktreeCreate {
            cwd: input.cwd.clone(),
            slug,
            mode,
        })?;
        let registered = self.register_created(&created, input, timestamp);
        if registered.is_ok() {
            return registered;
        }
        let cause = registered.expect_err("checked error");
        let owned = OwnedWorktree {
            path: created.worktree_path,
            repo_root: Some(created.repo_root),
        };
        match self.managed.remove(&owned) {
            Ok(()) => Err(cause),
            Err(rollback) => Err(WorktreesError::Rollback {
                cause: Box::new(cause),
                rollback,
            }),
        }
    }

    /// Archive one workspace or all active workspace records in a managed worktree.
    ///
    /// # Errors
    /// Returns invalid selection, ownership, registry, Git, or filesystem failures.
    pub fn archive(
        &self,
        input: &ArchiveWorktree,
        timestamp: &str,
    ) -> Result<ArchivedWorktree, WorktreesError> {
        let target = self.resolve_archive_target(input)?;
        let records = self.workspaces.list().map_err(map_registry)?;
        let (selected, ownership) = match input.scope {
            ArchiveScope::Workspace => {
                let workspace = if let Some(workspace_id) = &input.workspace_id {
                    records
                        .iter()
                        .find(|workspace| workspace.workspace_id == *workspace_id)
                        .cloned()
                } else {
                    let mut matches = records
                        .iter()
                        .filter(|workspace| is_active(workspace))
                        .filter(|workspace| same_path(&*self.managed, &workspace.cwd, &target))
                        .cloned()
                        .collect::<Vec<_>>();
                    matches.sort_by_key(|workspace| {
                        (
                            workspace.kind != PersistedWorkspaceKind::Worktree,
                            workspace.created_at.clone(),
                        )
                    });
                    matches.into_iter().next()
                };
                let ownership = workspace
                    .as_ref()
                    .and_then(|workspace| workspace_ownership(&*self.managed, workspace));
                let selected = workspace.into_iter().filter(is_active).collect::<Vec<_>>();
                (selected, ownership)
            }
            ArchiveScope::Worktree => {
                let owned = self
                    .managed
                    .owned(&target)
                    .map_err(|_| WorktreeError::NotAllowed)?;
                let selected = records
                    .iter()
                    .filter(|workspace| is_active(workspace))
                    .filter(|workspace| references(&*self.managed, workspace, &owned.path))
                    .cloned()
                    .collect::<Vec<_>>();
                (selected, Some(owned))
            }
        };

        let mut archived = Vec::with_capacity(selected.len());
        for workspace in selected {
            self.workspaces
                .archive(
                    &workspace.workspace_id,
                    timestamp,
                    &WorkspaceArchiveContext::default(),
                )
                .map_err(map_registry)?;
            archived.push(workspace.workspace_id);
        }

        if let Some(owned) = ownership {
            let remaining_reference =
                self.workspaces
                    .list()
                    .map_err(map_registry)?
                    .iter()
                    .any(|workspace| {
                        is_active(workspace) && references(&*self.managed, workspace, &owned.path)
                    });
            if input.scope == ArchiveScope::Worktree || !remaining_reference {
                self.managed.remove(&owned)?;
            }
        }
        Ok(ArchivedWorktree {
            workspace_ids: archived,
        })
    }

    fn register_created(
        &self,
        created: &CreatedManagedWorktree,
        input: &CreateWorktree,
        timestamp: &str,
    ) -> Result<CreatedWorkspace, WorktreesError> {
        let project = self.resolve_project(created, input.project_id.as_deref(), timestamp)?;
        let workspace = PersistedWorkspaceRecord {
            workspace_id: generate_workspace_id().map_err(|_| WorktreesError::Registry)?,
            project_id: project.project_id.clone(),
            cwd: created.workspace_cwd.clone(),
            kind: PersistedWorkspaceKind::Worktree,
            display_name: created.branch_name.clone(),
            title: input
                .first_agent_prompt
                .as_deref()
                .and_then(first_prompt_title),
            branch: Some(created.branch_name.clone()),
            worktree_root: Some(created.worktree_path.clone()),
            base_branch: created.comparison_base_ref.clone(),
            is_paseo_owned_worktree: true,
            main_repo_root: Some(created.repo_root.clone()),
            created_at: timestamp.to_owned(),
            updated_at: timestamp.to_owned(),
            archived_at: None,
            auto_archived_change_request_url: None,
            pinned_at: None,
            labels: None,
            untrusted_source: None,
        };
        self.workspaces
            .upsert(
                &workspace,
                WorkspaceMutationContext {
                    expects_initial_agent: input.expects_initial_agent.then_some(true),
                },
            )
            .map_err(map_registry)?;
        Ok(CreatedWorkspace { workspace, project })
    }

    fn resolve_project(
        &self,
        created: &CreatedManagedWorktree,
        project_id: Option<&str>,
        timestamp: &str,
    ) -> Result<PersistedProjectRecord, WorktreesError> {
        if let Some(project_id) = project_id {
            let project = self
                .projects
                .get(project_id)
                .map_err(map_registry)?
                .ok_or_else(|| WorktreesError::UnknownProject(project_id.to_owned()))?;
            if !is_active_project(&project) {
                return Err(WorktreesError::ArchivedProject(project_id.to_owned()));
            }
            return self.ensure_git_project(project, timestamp);
        }

        let source_project_id = self
            .workspaces
            .list()
            .map_err(map_registry)?
            .into_iter()
            .filter(is_active)
            .find(|workspace| {
                same_path(&*self.managed, &workspace.cwd, &created.source_cwd)
                    || same_path(&*self.managed, &workspace.cwd, &created.repo_root)
            })
            .map(|workspace| workspace.project_id);
        if let Some(project_id) = source_project_id
            && let Some(project) = self.projects.get(&project_id).map_err(map_registry)?
        {
            return self.ensure_git_project(project, timestamp);
        }

        let checkout = Checkout {
            cwd: created.repo_root.clone(),
            is_git: true,
            current_branch: None,
            remote_url: created.remote_url.clone(),
            worktree_root: Some(created.repo_root.clone()),
            is_paseo_owned_worktree: false,
            main_repo_root: None,
        };
        self.projects
            .get_or_create_active_by_root(&ActiveProjectInput {
                root_path: created.repo_root.clone(),
                kind: PersistedProjectKind::Git,
                display_name: basename(&created.repo_root),
                project_key: Some(derive_project_key(&checkout, &self.server_id)),
                timestamp: timestamp.to_owned(),
            })
            .map_err(map_registry)
    }

    fn ensure_git_project(
        &self,
        mut project: PersistedProjectRecord,
        timestamp: &str,
    ) -> Result<PersistedProjectRecord, WorktreesError> {
        if project.kind == PersistedProjectKind::Git {
            return Ok(project);
        }
        project.kind = PersistedProjectKind::Git;
        timestamp.clone_into(&mut project.updated_at);
        self.projects.upsert(&project).map_err(map_registry)?;
        Ok(project)
    }

    fn resolve_archive_target(&self, input: &ArchiveWorktree) -> Result<String, WorktreesError> {
        if let Some(path) = normalize_ref(input.worktree_path.as_deref()) {
            return Ok(path.to_owned());
        }
        if let Some(slug) = normalize_ref(input.worktree_slug.as_deref()) {
            let repo_root = normalize_ref(input.repo_root.as_deref()).ok_or_else(|| {
                WorktreeError::Invalid(
                    "repoRoot is required when worktreeSlug is supplied".to_owned(),
                )
            })?;
            return self
                .managed
                .path_for_slug(repo_root, slug)
                .map_err(Into::into);
        }
        if let (Some(repo_root), Some(branch)) = (
            normalize_ref(input.repo_root.as_deref()),
            normalize_ref(input.branch_name.as_deref()),
        ) {
            return self
                .managed
                .list(repo_root)?
                .into_iter()
                .find(|entry| entry.branch_name.as_deref() == Some(branch))
                .map(|entry| entry.path)
                .ok_or_else(|| {
                    WorktreeError::Invalid(format!("Paseo worktree not found for branch {branch}"))
                        .into()
                });
        }
        Err(WorktreeError::Invalid(
            "worktreePath, worktreeSlug, or repoRoot+branchName is required".to_owned(),
        )
        .into())
    }
}

fn workspace_ownership(
    managed: &dyn ManagedWorktrees,
    workspace: &PersistedWorkspaceRecord,
) -> Option<OwnedWorktree> {
    if workspace.is_paseo_owned_worktree
        && let (Some(worktree_root), Some(main_repo_root)) = (
            workspace.worktree_root.as_deref(),
            workspace.main_repo_root.as_deref(),
        )
    {
        let mut owned = managed.owned(worktree_root).ok()?;
        owned.repo_root = Some(main_repo_root.to_owned());
        return Some(owned);
    }
    if workspace.kind != PersistedWorkspaceKind::Worktree {
        return None;
    }
    managed
        .owned(workspace.worktree_root.as_deref().unwrap_or(&workspace.cwd))
        .ok()
}

fn references(
    managed: &dyn ManagedWorktrees,
    workspace: &PersistedWorkspaceRecord,
    root: &str,
) -> bool {
    workspace
        .worktree_root
        .as_deref()
        .is_some_and(|candidate| same_path(managed, candidate, root))
        || managed.contains(root, &workspace.cwd)
}

fn same_path(managed: &dyn ManagedWorktrees, left: &str, right: &str) -> bool {
    managed.contains(left, right) && managed.contains(right, left)
}

fn is_active(workspace: &PersistedWorkspaceRecord) -> bool {
    workspace.archived_at.as_ref().is_none_or(String::is_empty)
}

fn is_active_project(project: &PersistedProjectRecord) -> bool {
    project.archived_at.as_ref().is_none_or(String::is_empty)
}

fn normalize_ref(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

fn first_prompt_title(prompt: &str) -> Option<String> {
    let line = prompt
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())?;
    let normalized = line.split_whitespace().collect::<Vec<_>>().join(" ");
    let title = normalized.chars().take(60).collect::<String>();
    (!title.is_empty()).then_some(title)
}

fn slugify(input: &str) -> String {
    let mut slug = String::new();
    let mut separator = false;
    for character in input.chars().flat_map(char::to_lowercase) {
        if character.is_ascii_lowercase() || character.is_ascii_digit() {
            if separator && !slug.is_empty() {
                slug.push('-');
            }
            separator = false;
            slug.push(character);
        } else {
            separator = true;
        }
    }
    if slug.len() <= 50 {
        return slug;
    }
    let mut truncated = slug[..50].to_owned();
    if let Some(index) = truncated.rfind('-')
        && index > 25
    {
        truncated.truncate(index);
    }
    truncated.trim_end_matches('-').to_owned()
}

fn random_slug() -> Result<String, WorktreesError> {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut bytes = [0_u8; 4];
    getrandom::fill(&mut bytes).map_err(|_| WorktreesError::Registry)?;
    let mut slug = String::from("worktree-");
    for byte in bytes {
        slug.push(char::from(HEX[usize::from(byte >> 4)]));
        slug.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    Ok(slug)
}

const fn map_registry(_error: RegistryError) -> WorktreesError {
    WorktreesError::Registry
}

#[cfg(test)]
mod tests;
