//! Workspace setup and script coordination over registry and process ports.

use crate::model::registry::{PersistedWorkspaceRecord, UntrustedWorkspaceSource};
use crate::ports::registry::{RegistryError, WorkspaceRegistry};

pub use crate::ports::workspace_automation::{
    ScriptSnapshot, ScriptType, SetupCommandSnapshot, SetupLifecycle, SetupSnapshot,
    WorkspaceAutomationError, WorkspaceAutomationRuntime, WorkspacePlacement,
};

/// Setup status including a durable automation block when present.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SetupStatus {
    /// No setup has run and the workspace is trusted.
    Absent,
    /// Setup is blocked pending explicit approval.
    Blocked {
        /// Empty setup detail uses the current placement.
        placement: WorkspacePlacement,
        /// Untrusted checkout provenance.
        source: UntrustedWorkspaceSource,
    },
    /// Runtime snapshot for a started setup.
    Snapshot(SetupSnapshot),
}

/// Workspace automation failure.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum WorkspaceAutomationServiceError {
    /// Workspace is missing or archived.
    #[error("Workspace not found: {0}")]
    WorkspaceNotFound(String),
    /// Registry persistence failed.
    #[error("workspace registry operation failed")]
    Registry,
    /// Runtime/configuration failure.
    #[error(transparent)]
    Runtime(#[from] WorkspaceAutomationError),
}

/// Serialized setup/script application service.
#[derive(Debug)]
pub struct WorkspaceAutomation {
    workspaces: Box<dyn WorkspaceRegistry>,
    runtime: Box<dyn WorkspaceAutomationRuntime>,
}

impl WorkspaceAutomation {
    /// Compose the independent registry and process runtime.
    #[must_use]
    pub fn new(
        workspaces: Box<dyn WorkspaceRegistry>,
        runtime: Box<dyn WorkspaceAutomationRuntime>,
    ) -> Self {
        Self {
            workspaces,
            runtime,
        }
    }

    /// Read cached or durable blocked setup state.
    ///
    /// # Errors
    /// Returns registry failures.
    pub fn setup_status(
        &self,
        workspace_id: &str,
    ) -> Result<SetupStatus, WorkspaceAutomationServiceError> {
        if let Some(snapshot) = self.runtime.setup_snapshot(workspace_id) {
            return Ok(SetupStatus::Snapshot(snapshot));
        }
        let Some(workspace) = self.workspaces.get(workspace_id).map_err(map_registry)? else {
            return Ok(SetupStatus::Absent);
        };
        Ok(workspace
            .untrusted_source
            .clone()
            .map_or(SetupStatus::Absent, |source| SetupStatus::Blocked {
                placement: placement(&workspace),
                source,
            }))
    }

    /// Clear one durable automation block and start setup in the background.
    ///
    /// Trusted workspaces return false, matching Paseo's idempotent approval RPC.
    ///
    /// # Errors
    /// Returns missing workspace, registry, configuration, or runtime failures.
    pub fn approve_and_start_setup(
        &self,
        workspace_id: &str,
        timestamp: &str,
    ) -> Result<bool, WorkspaceAutomationServiceError> {
        let workspace = self.active_workspace(workspace_id)?;
        if workspace.untrusted_source.is_none() {
            return Ok(false);
        }
        let updated = self
            .workspaces
            .update(workspace_id, &|workspace| {
                let mut workspace = workspace.clone();
                workspace.untrusted_source = None;
                timestamp.clone_into(&mut workspace.updated_at);
                workspace
            })
            .map_err(map_registry)?
            .ok_or_else(|| {
                WorkspaceAutomationServiceError::WorkspaceNotFound(workspace_id.to_owned())
            })?;
        self.runtime
            .start_setup(&placement(&updated))
            .map_err(Into::into)
    }

    /// Start setup for a newly created trusted workspace.
    ///
    /// # Errors
    /// Returns missing workspace, registry, configuration, or runtime failures.
    pub fn start_created_setup(
        &self,
        workspace_id: &str,
    ) -> Result<bool, WorkspaceAutomationServiceError> {
        let workspace = self.active_workspace(workspace_id)?;
        if workspace.untrusted_source.is_some() {
            return Ok(false);
        }
        self.runtime
            .start_setup(&placement(&workspace))
            .map_err(Into::into)
    }

    /// List configured scripts with refreshed process state.
    ///
    /// # Errors
    /// Returns missing workspace, registry, or runtime failures.
    pub fn list_scripts(
        &self,
        workspace_id: &str,
    ) -> Result<Vec<ScriptSnapshot>, WorkspaceAutomationServiceError> {
        let workspace = self.active_workspace(workspace_id)?;
        self.runtime
            .list_scripts(&placement(&workspace))
            .map_err(Into::into)
    }

    /// Start one configured script.
    ///
    /// # Errors
    /// Returns missing workspace, blocked automation, registry, or runtime failures.
    pub fn start_script(
        &self,
        workspace_id: &str,
        script_name: &str,
    ) -> Result<ScriptSnapshot, WorkspaceAutomationServiceError> {
        let workspace = self.active_workspace(workspace_id)?;
        if workspace.untrusted_source.is_some() {
            return Err(WorkspaceAutomationError::Io(
                "Workspace automation is blocked until setup is approved".to_owned(),
            )
            .into());
        }
        self.runtime
            .start_script(&placement(&workspace), script_name)
            .map_err(Into::into)
    }

    /// Stop one running script.
    ///
    /// # Errors
    /// Returns missing workspace, registry, or runtime failures.
    pub fn stop_script(
        &self,
        workspace_id: &str,
        script_name: &str,
    ) -> Result<ScriptSnapshot, WorkspaceAutomationServiceError> {
        let workspace = self.active_workspace(workspace_id)?;
        self.runtime
            .stop_script(&placement(&workspace), script_name)
            .map_err(Into::into)
    }

    fn active_workspace(
        &self,
        workspace_id: &str,
    ) -> Result<PersistedWorkspaceRecord, WorkspaceAutomationServiceError> {
        self.workspaces
            .get(workspace_id)
            .map_err(map_registry)?
            .filter(|workspace| workspace.archived_at.as_ref().is_none_or(String::is_empty))
            .ok_or_else(|| {
                WorkspaceAutomationServiceError::WorkspaceNotFound(workspace_id.to_owned())
            })
    }
}

fn placement(workspace: &PersistedWorkspaceRecord) -> WorkspacePlacement {
    WorkspacePlacement {
        workspace_id: workspace.workspace_id.clone(),
        cwd: workspace.cwd.clone(),
        worktree_path: workspace
            .worktree_root
            .clone()
            .unwrap_or_else(|| workspace.cwd.clone()),
        repo_root: workspace
            .main_repo_root
            .clone()
            .unwrap_or_else(|| workspace.cwd.clone()),
        branch_name: workspace.branch.clone().unwrap_or_default(),
    }
}

const fn map_registry(_error: RegistryError) -> WorkspaceAutomationServiceError {
    WorkspaceAutomationServiceError::Registry
}

#[cfg(test)]
mod tests;
