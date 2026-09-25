//! Archived Workspace inspection and exact-placement recovery.
use server_metadata::model::registry::{
    PersistedProjectRecord, PersistedWorkspaceKind, PersistedWorkspaceRecord,
};
use server_metadata::ports::registry::{ProjectRegistry, RegistryError, WorkspaceRegistry};

use crate::ports::workspace_recovery::{ArchivedWorktreeRestore, WorkspaceRecoveryRuntime};
/// Archived Workspace recovery action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceRecoveryAction {
    /// The exact Workspace directory still exists; only registry records are reopened.
    Unarchive,
    /// A deleted managed worktree must be recreated before records are reopened.
    Restore,
}

/// Stable reasons an archived Workspace cannot be recovered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceRecoveryUnavailableReason {
    /// No Workspace record exists.
    WorkspaceNotFound,
    /// The Workspace is already active.
    WorkspaceNotArchived,
    /// Its Project record no longer exists.
    ProjectNotFound,
    /// The repository needed to recreate a worktree is absent.
    ProjectDirectoryMissing,
    /// A non-worktree directory was deleted.
    WorkspaceDirectoryMissing,
    /// A deleted worktree has no saved branch.
    WorktreeBranchMissing,
}

/// Read-only recovery inspection result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkspaceRecoveryState {
    /// Recovery can proceed.
    Recoverable {
        /// Workspace identity.
        workspace_id: String,
        /// Current stored display name.
        workspace_name: String,
        /// Required recovery action.
        action: WorkspaceRecoveryAction,
        /// Saved branch, when present.
        branch: Option<String>,
    },
    /// Recovery cannot safely proceed.
    Unavailable {
        /// Requested Workspace identity.
        workspace_id: String,
        /// Stable unavailable category.
        reason: WorkspaceRecoveryUnavailableReason,
        /// User-facing explanation aligned with Paseo.
        message: String,
    },
}

/// Successful recovered placement used by the API to publish a Workspace update.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveredWorkspace {
    /// Reopened Workspace record.
    pub workspace: PersistedWorkspaceRecord,
    /// Reopened owning Project record.
    pub project: PersistedProjectRecord,
    /// Recovery action that was performed.
    pub action: WorkspaceRecoveryAction,
}

/// Archived Workspace recovery failure.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum WorkspaceRecoveryError {
    /// Workspace registry persistence failed.
    #[error("Workspace registry failed")]
    WorkspaceRegistry,
    /// Project registry persistence failed.
    #[error("Project registry failed")]
    ProjectRegistry,
    /// Inspection determined that recovery is unavailable.
    #[error("{0}")]
    RecoveryUnavailable(String),
    /// Local Git or filesystem recovery failed.
    #[error("{0}")]
    RecoveryRuntime(String),
}
#[derive(Debug)]
enum RecoveryPlan {
    Unarchive {
        workspace: PersistedWorkspaceRecord,
        project: PersistedProjectRecord,
    },
    Restore {
        workspace: PersistedWorkspaceRecord,
        project: PersistedProjectRecord,
        source_repo_root: String,
    },
}

/// Recovery coordinator using the host's shared Project and Workspace registries.
#[derive(Debug)]
pub struct WorkspaceRecovery {
    workspaces: Box<dyn WorkspaceRegistry>,
    projects: Box<dyn ProjectRegistry>,
    recovery: Box<dyn WorkspaceRecoveryRuntime>,
}
impl WorkspaceRecovery {
    /// Compose durable registries and the local placement recovery runtime.
    #[must_use]
    pub fn new(
        workspaces: Box<dyn WorkspaceRegistry>,
        projects: Box<dyn ProjectRegistry>,
        recovery: Box<dyn WorkspaceRecoveryRuntime>,
    ) -> Self {
        Self {
            workspaces,
            projects,
            recovery,
        }
    }
    /// Inspect whether an archived Workspace can be reopened or recreated.
    ///
    /// # Errors
    /// Returns registry failures; expected unavailable states are returned as values.
    pub fn inspect_recovery(
        &self,
        workspace_id: &str,
    ) -> Result<WorkspaceRecoveryState, WorkspaceRecoveryError> {
        match self.resolve_recovery(workspace_id)? {
            Ok(plan) => Ok(recovery_state(&plan)),
            Err(state) => Ok(state),
        }
    }

    /// Recover one archived Workspace, recreating its exact managed worktree when needed.
    ///
    /// # Errors
    /// Returns unavailable, Git/filesystem, or durable registry failures. The Workspace remains
    /// archived when worktree recreation itself fails.
    pub fn restore(
        &self,
        workspace_id: &str,
        updated_at: &str,
    ) -> Result<RecoveredWorkspace, WorkspaceRecoveryError> {
        let plan = self
            .resolve_recovery(workspace_id)?
            .map_err(|state| match state {
                WorkspaceRecoveryState::Unavailable { message, .. } => {
                    WorkspaceRecoveryError::RecoveryUnavailable(message)
                }
                WorkspaceRecoveryState::Recoverable { .. } => unreachable!(),
            })?;
        let (workspace, project, action) = match plan {
            RecoveryPlan::Unarchive { workspace, project } => {
                (workspace, project, WorkspaceRecoveryAction::Unarchive)
            }
            RecoveryPlan::Restore {
                workspace,
                project,
                source_repo_root,
            } => {
                let branch = workspace.branch.clone().ok_or_else(|| {
                    WorkspaceRecoveryError::RecoveryUnavailable(
                        "The archived worktree has no branch recorded, so it cannot be restored."
                            .to_owned(),
                    )
                })?;
                self.recovery
                    .restore_worktree(&ArchivedWorktreeRestore {
                        source_repo_root,
                        previous_worktree_root: workspace
                            .worktree_root
                            .clone()
                            .unwrap_or_else(|| workspace.cwd.clone()),
                        workspace_cwd: workspace.cwd.clone(),
                        branch,
                        base_ref: workspace.base_branch.clone(),
                    })
                    .map_err(|error| WorkspaceRecoveryError::RecoveryRuntime(error.to_string()))?;
                (workspace, project, WorkspaceRecoveryAction::Restore)
            }
        };
        let project = if is_archived(project.archived_at.as_deref()) {
            self.projects
                .update(&project.project_id, &|current| {
                    let mut next = current.clone();
                    next.archived_at = None;
                    updated_at.clone_into(&mut next.updated_at);
                    next
                })
                .map_err(map_project_error)?
                .ok_or(WorkspaceRecoveryError::ProjectRegistry)?
        } else {
            project
        };
        let workspace = self
            .workspaces
            .update(&workspace.workspace_id, &|current| {
                let mut next = current.clone();
                next.archived_at = None;
                updated_at.clone_into(&mut next.updated_at);
                next
            })
            .map_err(map_workspace_error)?
            .ok_or(WorkspaceRecoveryError::WorkspaceRegistry)?;
        Ok(RecoveredWorkspace {
            workspace,
            project,
            action,
        })
    }

    fn resolve_recovery(
        &self,
        workspace_id: &str,
    ) -> Result<Result<RecoveryPlan, WorkspaceRecoveryState>, WorkspaceRecoveryError> {
        let Some(workspace) = self
            .workspaces
            .get(workspace_id)
            .map_err(map_workspace_error)?
        else {
            return Ok(Err(unavailable(
                workspace_id,
                WorkspaceRecoveryUnavailableReason::WorkspaceNotFound,
                "This workspace is no longer known to the host.",
            )));
        };
        if !is_archived(workspace.archived_at.as_deref()) {
            return Ok(Err(unavailable(
                workspace_id,
                WorkspaceRecoveryUnavailableReason::WorkspaceNotArchived,
                "This workspace is not archived, but it is unavailable from the host.",
            )));
        }
        let Some(project) = self
            .projects
            .get(&workspace.project_id)
            .map_err(map_project_error)?
        else {
            return Ok(Err(unavailable(
                workspace_id,
                WorkspaceRecoveryUnavailableReason::ProjectNotFound,
                "The project for this archived workspace no longer exists.",
            )));
        };
        if self.recovery.is_directory(&workspace.cwd) {
            return Ok(Ok(RecoveryPlan::Unarchive { workspace, project }));
        }
        if workspace.kind != PersistedWorkspaceKind::Worktree {
            return Ok(Err(unavailable(
                workspace_id,
                WorkspaceRecoveryUnavailableReason::WorkspaceDirectoryMissing,
                "The archived workspace directory no longer exists and cannot be recreated.",
            )));
        }
        if workspace.branch.as_deref().is_none_or(str::is_empty) {
            return Ok(Err(unavailable(
                workspace_id,
                WorkspaceRecoveryUnavailableReason::WorktreeBranchMissing,
                "The archived worktree has no branch recorded, so it cannot be restored.",
            )));
        }
        let source_repo_root = workspace
            .main_repo_root
            .clone()
            .unwrap_or_else(|| project.root_path.clone());
        if !self.recovery.is_directory(&source_repo_root) {
            return Ok(Err(unavailable(
                workspace_id,
                WorkspaceRecoveryUnavailableReason::ProjectDirectoryMissing,
                "The source repository needed to restore this worktree no longer exists.",
            )));
        }
        Ok(Ok(RecoveryPlan::Restore {
            workspace,
            project,
            source_repo_root,
        }))
    }
}
fn unavailable(
    workspace_id: &str,
    reason: WorkspaceRecoveryUnavailableReason,
    message: &str,
) -> WorkspaceRecoveryState {
    WorkspaceRecoveryState::Unavailable {
        workspace_id: workspace_id.to_owned(),
        reason,
        message: message.to_owned(),
    }
}

fn recovery_state(plan: &RecoveryPlan) -> WorkspaceRecoveryState {
    let (workspace, action) = match plan {
        RecoveryPlan::Unarchive { workspace, .. } => {
            (workspace, WorkspaceRecoveryAction::Unarchive)
        }
        RecoveryPlan::Restore { workspace, .. } => (workspace, WorkspaceRecoveryAction::Restore),
    };
    WorkspaceRecoveryState::Recoverable {
        workspace_id: workspace.workspace_id.clone(),
        workspace_name: workspace.display_name().to_owned(),
        action,
        branch: workspace.branch.clone(),
    }
}

fn is_archived(value: Option<&str>) -> bool {
    value.is_some_and(|value| !value.is_empty())
}
const fn map_workspace_error(_: RegistryError) -> WorkspaceRecoveryError {
    WorkspaceRecoveryError::WorkspaceRegistry
}
const fn map_project_error(_: RegistryError) -> WorkspaceRecoveryError {
    WorkspaceRecoveryError::ProjectRegistry
}
#[cfg(test)]
mod tests;
