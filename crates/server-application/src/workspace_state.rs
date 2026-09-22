//! Workspace attention and archived-placement recovery use cases.

use std::cmp::Ordering;
use std::collections::BTreeMap;

use chrono::{DateTime, Duration, SecondsFormat, Utc};
use server_domain::agent_runtime::{
    AgentAttentionReason, AgentRuntimeStatus, PersistedAgentRuntimeRecord,
};
use server_domain::registry::{
    PersistedProjectRecord, PersistedWorkspaceKind, PersistedWorkspaceRecord,
};
use server_ports::agent_runtime::{AgentRuntimeRegistry, AgentRuntimeRegistryError};
use server_ports::registry::{ProjectRegistry, RegistryError, WorkspaceRegistry};
use server_ports::workspace_recovery::{ArchivedWorktreeRestore, WorkspaceRecoveryRuntime};

const PARENT_AGENT_ID_LABEL: &str = "paseo.parent-agent-id";

/// Result for one Workspace in a clear-attention batch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceAttentionResult {
    /// Requested Workspace identity.
    pub workspace_id: String,
    /// Agents whose attention was durably cleared before this result completed.
    pub cleared_agent_ids: Vec<String>,
    /// Whether every eligible Agent in this Workspace was processed.
    pub success: bool,
    /// Inline error text for this Workspace.
    pub error: Option<String>,
}

/// Aggregate clear-attention result matching Paseo's batch semantics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceAttentionBatch {
    /// Flattened cleared Agent identities in request and registry order.
    pub cleared_agent_ids: Vec<String>,
    /// One result for every requested Workspace, including failures.
    pub results: Vec<WorkspaceAttentionResult>,
    /// True only when every Workspace succeeded.
    pub success: bool,
    /// Semicolon-separated errors, or none when the whole batch succeeded.
    pub error: Option<String>,
}

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

/// Workspace state operation failure.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum WorkspaceStateError {
    /// Workspace is missing or archived for an attention mutation.
    #[error("Workspace not found: {0}")]
    WorkspaceNotFound(String),
    /// No eligible finished root Agent exists.
    #[error("Workspace has no finished agent to mark unread: {0}")]
    NoFinishedAgent(String),
    /// The selected Agent changed after candidate selection.
    #[error("Agent is no longer finished and read: {0}")]
    AgentNoLongerFinished(String),
    /// Agent runtime persistence failed.
    #[error("Agent runtime registry failed")]
    AgentRegistry,
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

/// Coordinates Workspace attention and recovery without depending on Provider execution.
#[derive(Debug)]
pub struct WorkspaceState {
    agents: Box<dyn AgentRuntimeRegistry>,
    workspaces: Box<dyn WorkspaceRegistry>,
    projects: Box<dyn ProjectRegistry>,
    recovery: Box<dyn WorkspaceRecoveryRuntime>,
}

impl WorkspaceState {
    /// Compose independent durable registries and the local recovery adapter.
    #[must_use]
    pub fn new(
        agents: Box<dyn AgentRuntimeRegistry>,
        workspaces: Box<dyn WorkspaceRegistry>,
        projects: Box<dyn ProjectRegistry>,
        recovery: Box<dyn WorkspaceRecoveryRuntime>,
    ) -> Self {
        Self {
            agents,
            workspaces,
            projects,
            recovery,
        }
    }

    /// Clear non-permission attention for active Agents owned by each requested Workspace.
    ///
    /// Each Workspace is independent: a later failure does not hide earlier durable updates.
    #[must_use]
    pub fn clear_attention(
        &self,
        workspace_ids: &[String],
        updated_at: &str,
    ) -> WorkspaceAttentionBatch {
        let agents = match self.agents.list().map_err(map_agent_error) {
            Ok(agents) => agents,
            Err(error) => {
                return attention_batch(
                    workspace_ids
                        .iter()
                        .map(|workspace_id| WorkspaceAttentionResult {
                            workspace_id: workspace_id.clone(),
                            cleared_agent_ids: Vec::new(),
                            success: false,
                            error: Some(error.to_string()),
                        })
                        .collect(),
                );
            }
        };
        let results = workspace_ids
            .iter()
            .map(|workspace_id| self.clear_workspace_attention(workspace_id, &agents, updated_at))
            .collect();
        attention_batch(results)
    }

    /// Mark the newest finished, read root Agent in an active Workspace as unread.
    ///
    /// # Errors
    /// Returns an inline business error for missing Workspace/candidate and a categorized registry
    /// failure when durable state cannot be read or changed.
    pub fn mark_unread(
        &self,
        workspace_id: &str,
        updated_at: &str,
    ) -> Result<String, WorkspaceStateError> {
        self.require_active_workspace(workspace_id)?;
        let agents = self.agents.list().map_err(map_agent_error)?;
        let by_id = agents
            .iter()
            .filter(|agent| !agent.internal)
            .map(|agent| (agent.id.as_str(), agent))
            .collect::<BTreeMap<_, _>>();
        let mut candidates = agents
            .iter()
            .filter(|agent| !agent.internal && !is_archived(agent.archived_at.as_deref()))
            .filter(|agent| agent.workspace_id.as_deref() == Some(workspace_id))
            .filter(|agent| workspace_root_id(agent, &by_id) == Some(agent.id.as_str()))
            .filter(|agent| {
                matches!(
                    agent.last_status,
                    AgentRuntimeStatus::Idle | AgentRuntimeStatus::Closed
                )
            })
            .filter(|agent| !agent.requires_attention)
            .collect::<Vec<_>>();
        candidates.sort_by(|left, right| {
            compare_agent_updated_at(right, left).then_with(|| left.id.cmp(&right.id))
        });
        let candidate = candidates
            .first()
            .ok_or_else(|| WorkspaceStateError::NoFinishedAgent(workspace_id.to_owned()))?;
        let agent_id = candidate.id.clone();
        let next_updated_at = monotonic_timestamp(&candidate.updated_at, updated_at);
        let updated = self
            .agents
            .update(&agent_id, &|current| {
                if current.internal
                    || is_archived(current.archived_at.as_deref())
                    || current.requires_attention
                    || !matches!(
                        current.last_status,
                        AgentRuntimeStatus::Idle | AgentRuntimeStatus::Closed
                    )
                {
                    return current.clone();
                }
                let mut next = current.clone();
                next.updated_at.clone_from(&next_updated_at);
                next.requires_attention = true;
                next.attention_reason = Some(AgentAttentionReason::Finished);
                next.attention_timestamp = Some(next_updated_at.clone());
                next
            })
            .map_err(map_agent_error)?
            .ok_or_else(|| WorkspaceStateError::AgentNoLongerFinished(agent_id.clone()))?;
        if !updated.requires_attention
            || updated.attention_reason != Some(AgentAttentionReason::Finished)
        {
            return Err(WorkspaceStateError::AgentNoLongerFinished(agent_id));
        }
        Ok(updated.id)
    }

    /// Inspect whether an archived Workspace can be reopened or recreated.
    ///
    /// # Errors
    /// Returns registry failures; expected unavailable states are returned as values.
    pub fn inspect_recovery(
        &self,
        workspace_id: &str,
    ) -> Result<WorkspaceRecoveryState, WorkspaceStateError> {
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
    ) -> Result<RecoveredWorkspace, WorkspaceStateError> {
        let plan = self
            .resolve_recovery(workspace_id)?
            .map_err(|state| match state {
                WorkspaceRecoveryState::Unavailable { message, .. } => {
                    WorkspaceStateError::RecoveryUnavailable(message)
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
                    WorkspaceStateError::RecoveryUnavailable(
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
                    .map_err(|error| WorkspaceStateError::RecoveryRuntime(error.to_string()))?;
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
                .ok_or(WorkspaceStateError::ProjectRegistry)?
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
            .ok_or(WorkspaceStateError::WorkspaceRegistry)?;
        Ok(RecoveredWorkspace {
            workspace,
            project,
            action,
        })
    }

    fn clear_workspace_attention(
        &self,
        workspace_id: &str,
        agents: &[PersistedAgentRuntimeRecord],
        updated_at: &str,
    ) -> WorkspaceAttentionResult {
        let mut result = WorkspaceAttentionResult {
            workspace_id: workspace_id.to_owned(),
            cleared_agent_ids: Vec::new(),
            success: false,
            error: None,
        };
        if let Err(error) = self.require_active_workspace(workspace_id) {
            result.error = Some(error.to_string());
            return result;
        }
        for agent in agents.iter().filter(|agent| {
            !agent.internal
                && !is_archived(agent.archived_at.as_deref())
                && agent.workspace_id.as_deref() == Some(workspace_id)
                && agent.requires_attention
                && agent.attention_reason != Some(AgentAttentionReason::Permission)
        }) {
            let next_updated_at = monotonic_timestamp(&agent.updated_at, updated_at);
            match self.agents.update(&agent.id, &|current| {
                let mut next = current.clone();
                next.updated_at.clone_from(&next_updated_at);
                next.requires_attention = false;
                next.attention_reason = None;
                next.attention_timestamp = None;
                next
            }) {
                Ok(Some(_)) => result.cleared_agent_ids.push(agent.id.clone()),
                Ok(None) => {
                    result.error = Some(
                        WorkspaceStateError::AgentNoLongerFinished(agent.id.clone()).to_string(),
                    );
                    return result;
                }
                Err(error) => {
                    result.error = Some(map_agent_error(error).to_string());
                    return result;
                }
            }
        }
        result.success = true;
        result
    }

    fn require_active_workspace(
        &self,
        workspace_id: &str,
    ) -> Result<PersistedWorkspaceRecord, WorkspaceStateError> {
        let workspace = self
            .workspaces
            .get(workspace_id)
            .map_err(map_workspace_error)?
            .ok_or_else(|| WorkspaceStateError::WorkspaceNotFound(workspace_id.to_owned()))?;
        if is_archived(workspace.archived_at.as_deref()) {
            return Err(WorkspaceStateError::WorkspaceNotFound(
                workspace_id.to_owned(),
            ));
        }
        Ok(workspace)
    }

    fn resolve_recovery(
        &self,
        workspace_id: &str,
    ) -> Result<Result<RecoveryPlan, WorkspaceRecoveryState>, WorkspaceStateError> {
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

fn attention_batch(results: Vec<WorkspaceAttentionResult>) -> WorkspaceAttentionBatch {
    let cleared_agent_ids = results
        .iter()
        .flat_map(|result| result.cleared_agent_ids.iter().cloned())
        .collect();
    let errors = results
        .iter()
        .filter_map(|result| {
            (!result.success)
                .then_some(result.error.as_deref())
                .flatten()
        })
        .collect::<Vec<_>>();
    WorkspaceAttentionBatch {
        cleared_agent_ids,
        success: errors.is_empty(),
        error: (!errors.is_empty()).then(|| errors.join("; ")),
        results,
    }
}

fn workspace_root_id<'a>(
    agent: &'a PersistedAgentRuntimeRecord,
    agents: &BTreeMap<&str, &'a PersistedAgentRuntimeRecord>,
) -> Option<&'a str> {
    let mut seen = std::collections::BTreeSet::from([agent.id.as_str()]);
    let mut current = agent;
    loop {
        let Some(parent_id) = current
            .labels
            .get(PARENT_AGENT_ID_LABEL)
            .map(String::as_str)
            .filter(|parent_id| !parent_id.is_empty())
        else {
            return Some(current.id.as_str());
        };
        if !seen.insert(parent_id) {
            return None;
        }
        let parent = agents.get(parent_id)?;
        if parent.workspace_id != current.workspace_id {
            return Some(current.id.as_str());
        }
        current = parent;
    }
}

fn compare_agent_updated_at(
    left: &PersistedAgentRuntimeRecord,
    right: &PersistedAgentRuntimeRecord,
) -> Ordering {
    let left = effective_timestamp(left);
    let right = effective_timestamp(right);
    match (parse_timestamp(left), parse_timestamp(right)) {
        (Some(left), Some(right)) => left.cmp(&right),
        _ => left.cmp(right),
    }
}

fn effective_timestamp(record: &PersistedAgentRuntimeRecord) -> &str {
    let updated = parse_timestamp(&record.updated_at);
    let activity = record.last_activity_at.as_deref().and_then(parse_timestamp);
    if activity.is_some_and(|activity| updated.is_none_or(|updated| activity > updated)) {
        record
            .last_activity_at
            .as_deref()
            .unwrap_or(&record.updated_at)
    } else {
        &record.updated_at
    }
}

fn monotonic_timestamp(previous: &str, proposed: &str) -> String {
    let previous = parse_timestamp(previous);
    let parsed_proposed = parse_timestamp(proposed);
    match (previous, parsed_proposed) {
        (Some(previous), Some(parsed_proposed)) if parsed_proposed <= previous => (previous
            + Duration::milliseconds(1))
        .with_timezone(&Utc)
        .to_rfc3339_opts(SecondsFormat::Millis, true),
        (_, Some(parsed_proposed)) => parsed_proposed
            .with_timezone(&Utc)
            .to_rfc3339_opts(SecondsFormat::Millis, true),
        _ => proposed.to_owned(),
    }
}

fn parse_timestamp(value: &str) -> Option<DateTime<chrono::FixedOffset>> {
    DateTime::parse_from_rfc3339(value).ok()
}

fn is_archived(archived_at: Option<&str>) -> bool {
    archived_at.is_some_and(|value| !value.is_empty())
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

const fn map_agent_error(_: AgentRuntimeRegistryError) -> WorkspaceStateError {
    WorkspaceStateError::AgentRegistry
}

const fn map_workspace_error(_: RegistryError) -> WorkspaceStateError {
    WorkspaceStateError::WorkspaceRegistry
}

const fn map_project_error(_: RegistryError) -> WorkspaceStateError {
    WorkspaceStateError::ProjectRegistry
}

#[cfg(test)]
mod tests;
