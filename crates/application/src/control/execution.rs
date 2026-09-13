//! Command admission and CAS commits; execution continuations run only after persistence.
use crate::control::LocalControlService;
use crate::control::errors::{error, store_error};
use crate::control::runs::finalization::{
    InvocationGuard, WorkspaceRunControl, WorkspaceRunControlGuard,
};
use ait_contracts::{ApiError, Command, CommandResult, RunView};
use ait_domain::ErrorCode;
use ait_ports::ControlStoreError;
use std::path::PathBuf;
use std::sync::Arc;

/// Internal continuation produced by a command, consumed only after its state
/// is committed. A public `RunView` is a result snapshot, never an execution signal.
pub(in crate::control) enum CommandOutcome {
    Ready(Box<CommandResult>),
    ExecuteWorkspaceRun(Box<RunView>),
}

impl CommandOutcome {
    pub(in crate::control) fn for_new_run(run: RunView) -> Self {
        Self::ExecuteWorkspaceRun(Box::new(run))
    }
}

impl LocalControlService {
    pub(in crate::control) async fn try_submit(
        self: &Arc<Self>,
        command: Command,
    ) -> Result<CommandResult, ApiError> {
        if !matches!(
            command,
            Command::SendMessage { .. }
                | Command::ForkSession { .. }
                | Command::DeriveSession { .. }
        ) {
            return Err(error(
                ErrorCode::InvalidConfiguration,
                "only interactive Run commands support asynchronous submission",
                false,
            ));
        }
        let mut session_admission = self.acquire_session(&command)?;
        let derive_source_locked = session_admission.derive_source_locked();
        let workspace_lease = self.acquire_workspace_write(&command).await?;
        let has_workspace_lease = workspace_lease.is_some();
        match self
            .commit_with_finalization_gate(command, has_workspace_lease, derive_source_locked)
            .await?
        {
            CommandOutcome::ExecuteWorkspaceRun(run) => {
                let accepted = (*run).clone();
                if let Some(session_id) = &accepted.session_id {
                    session_admission.retain_for_session(session_id);
                }
                let run_id = run.id.clone();
                let control = Arc::new(WorkspaceRunControl::new());
                let invocation = InvocationGuard::new(
                    Arc::clone(&self.cancellations),
                    &run_id,
                    control.cancellation.clone(),
                );
                let control_guard = WorkspaceRunControlGuard::new(
                    Arc::clone(&self.workspace_run_controls),
                    &run_id,
                    &control,
                );
                let service = Arc::clone(self);
                tokio::spawn(async move {
                    let _owned = (
                        session_admission,
                        workspace_lease,
                        invocation,
                        control_guard,
                    );
                    let _ = service.supervise_workspace_agent(run_id, control).await;
                });
                Ok(CommandResult::Run(accepted))
            }
            CommandOutcome::Ready(_) => unreachable!("interactive submission creates a Run"),
        }
    }

    pub(in crate::control) async fn try_execute(
        &self,
        command: Command,
    ) -> Result<CommandResult, ApiError> {
        if matches!(
            command,
            Command::GetRun { .. }
                | Command::ExportProject { .. }
                | Command::GetSettings
                | Command::ListProjects
                | Command::ListAgents
                | Command::ListAgentProviders
                | Command::ListSessions { .. }
                | Command::ListMessages { .. }
                | Command::ListRuns { .. }
                | Command::ListCrons
        ) {
            let loaded = self.read_command_records(&command).await?;
            return loaded.read(command);
        }

        // A Session request never waits behind a running turn: reject it immediately.
        let mut session_admission = self.acquire_session(&command)?;
        let derive_source_locked = session_admission.derive_source_locked();
        if let Command::SaveAgentProvider { provider, secret } = command {
            return self.save_provider(provider, secret).await;
        }
        if let Command::DiscoverProviderModels { provider, secret } = command {
            return self.discover_provider_models(provider, secret).await;
        }
        if let Command::RefreshProviderModels { provider_id } = command {
            return self.refresh_provider(&provider_id).await;
        }
        // Codex workspace requests for the same canonical Git root are serialized
        // before the user Message captures HEAD. API-only providers still validate
        // the same Run permission profile but cannot require this lease until a
        // host-owned tool bridge gives them workspace side effects.
        let workspace_lease = self.acquire_workspace_write(&command).await?;
        let has_workspace_lease = workspace_lease.is_some();
        // Commit retries may reapply state changes, but never repeat an external
        // Agent invocation. Only the command that created the Run can request it.
        let outcome = self
            .commit_with_finalization_gate(command, has_workspace_lease, derive_source_locked)
            .await?;
        match outcome {
            CommandOutcome::Ready(result) => Ok(*result),
            CommandOutcome::ExecuteWorkspaceRun(run) => {
                let run_id = run.id.clone();
                if let Some(session_id) = &run.session_id {
                    session_admission.retain_for_session(session_id);
                }
                let control = Arc::new(WorkspaceRunControl::new());
                let invocation = InvocationGuard::new(
                    Arc::clone(&self.cancellations),
                    &run_id,
                    control.cancellation.clone(),
                );
                let control_guard = WorkspaceRunControlGuard::new(
                    Arc::clone(&self.workspace_run_controls),
                    &run_id,
                    &control,
                );
                let service = self.clone();
                let (sender, receiver) = tokio::sync::oneshot::channel();
                tokio::spawn(async move {
                    // These guards deliberately live in the transport-independent
                    // supervisor until terminal persistence has completed.
                    let _owned = (
                        session_admission,
                        workspace_lease,
                        invocation,
                        control_guard,
                    );
                    let result = service.supervise_workspace_agent(run_id, control).await;
                    let _ = sender.send(result);
                });
                receiver
                    .await
                    .map_err(|_| {
                        error(
                            ErrorCode::ProviderFailed,
                            "workspace execution supervisor stopped before returning a result",
                            true,
                        )
                    })?
                    .map(CommandResult::Run)
            }
        }
    }

    pub(in crate::control) async fn commit_command(
        &self,
        command: Command,
        has_workspace_lease: bool,
        derive_source_locked: bool,
    ) -> Result<CommandOutcome, ApiError> {
        let mut created_workdir = None;
        let mut created_session_worktrees = Vec::new();
        self.commit_command_inner(
            command,
            has_workspace_lease,
            derive_source_locked,
            &mut created_workdir,
            &mut created_session_worktrees,
        )
            .await
            .map_err(|mut failure| {
                if let Some(path) = created_workdir {
                    // Filesystem and SQLite are separate commit domains. Preserve
                    // the new directory, including anything written concurrently.
                    failure.message = format!("{} Directory retained at {}. Inspect it before explicitly registering it or choosing another name.", failure.message, path.display());
                    failure.retryable = false;
                }
                if !created_session_worktrees.is_empty() {
                    let retained = created_session_worktrees
                        .iter()
                        .map(|path| path.display().to_string())
                        .collect::<Vec<_>>()
                        .join(", ");
                    failure.message = format!(
                        "{} Session worktree retained at {retained}; inspect it before retrying.",
                        failure.message
                    );
                    failure.retryable = false;
                }
                failure
            })
    }

    async fn commit_command_inner(
        &self,
        mut command: Command,
        has_workspace_lease: bool,
        derive_source_locked: bool,
        created_workdir: &mut Option<PathBuf>,
        created_session_worktrees: &mut Vec<PathBuf>,
    ) -> Result<CommandOutcome, ApiError> {
        let initial = self.read_command_records(&command).await?;
        self.allocate_project_directory(&initial, &mut command, created_workdir)?;
        initial.check_admission(&command)?;
        initial.prepare(&command, created_session_worktrees)?;
        let prepared_project = match &command {
            Command::RegisterProject { workdir, .. } => {
                Some(crate::control::project::git::prepare_project(
                    workdir.as_deref().expect("allocated workdir"),
                )?)
            }
            Command::ImportProject { workdir, .. } => {
                Some(crate::control::project::git::prepare_project(workdir)?)
            }
            _ => None,
        };
        if let Some(prepared) = &prepared_project {
            match &mut command {
                Command::RegisterProject { workdir, .. } => {
                    *workdir = Some(prepared.workdir.clone());
                }
                Command::ImportProject { workdir, .. } => workdir.clone_from(&prepared.workdir),
                _ => unreachable!("Project preparation"),
            }
        }
        let initial = if prepared_project.is_some() {
            // Include the allocated/canonical path in the uniqueness read before the CAS.
            self.read_command_records(&command).await?
        } else {
            initial
        };
        let preparation_key = initial.preparation_key();
        for attempt in 0..4 {
            let loaded = if attempt == 0 {
                None
            } else {
                Some(self.read_command_records(&command).await?)
            };
            let loaded = loaded.as_ref().unwrap_or(&initial);
            loaded.check_admission(&command)?;
            if loaded.preparation_key() != preparation_key {
                return Err(error(
                    ErrorCode::RunQueueConflict,
                    "prepared command references changed; retry the request",
                    true,
                ));
            }
            if !has_workspace_lease
                && loaded
                    .workspace_path(&command, self.permission_limits)?
                    .is_some()
            {
                return Err(error(
                    ErrorCode::ProjectWorkspaceBusy,
                    "Agent configuration changed to a workspace-writing provider during admission; retry the request",
                    true,
                ));
            }
            // Git observation is read-only; directory/worktree creation is above the retry loop.
            let git_baseline = loaded.git_baseline(&command)?;
            let commit = loaded.reduce(
                command.clone(),
                git_baseline.as_ref(),
                self.permission_limits,
                derive_source_locked,
                prepared_project.as_ref(),
            )?;
            match self
                .store
                .apply(commit.revision, commit.changes, commit.events)
                .await
            {
                Ok(_) => {
                    let result = commit.outcome;
                    if matches!(
                        command,
                        Command::ResolveNativeApproval { .. } | Command::CancelRun { .. }
                    ) && let CommandOutcome::Ready(value) = &result
                        && let CommandResult::Run(run) = value.as_ref()
                    {
                        self.notify_approval_waiters(run);
                    }
                    return Ok(result);
                }
                Err(ControlStoreError::Conflict) => {}
                Err(error) => return Err(store_error(error)),
            }
        }
        Err(error(
            ErrorCode::RunQueueConflict,
            "concurrent state update did not settle",
            true,
        ))
    }
    fn allocate_project_directory(
        &self,
        initial: &crate::control::state::commands::CommandTransaction,
        command: &mut Command,
        created_workdir: &mut Option<PathBuf>,
    ) -> Result<(), ApiError> {
        if let Command::RegisterProject {
            id,
            name,
            workdir,
            repo_url,
        } = command
        {
            initial.validate_registration(id, name, repo_url)?;
            if workdir.is_none() {
                let creator = self.project_directory_creator.as_ref().ok_or_else(|| {
                    error(
                        ErrorCode::ProjectDefaultDirectoryUnavailable,
                        "The host has no default Project directory configured; specify a workdir.",
                        false,
                    )
                })?;
                let path = creator
                    .create_workdir(name)
                    .map_err(|failure| error(failure.code, failure.message, failure.retryable))?;
                *created_workdir = Some(path.clone());
                *workdir = Some(
                    path.to_str()
                        .ok_or_else(|| {
                            error(
                                ErrorCode::InvalidProject,
                                "Project directory must have a UTF-8 path",
                                false,
                            )
                        })?
                        .to_owned(),
                );
                // Keep this allocated path across CAS retries. Another request
                // still has to allocate independently and will fail at mkdir.
            }
        }

        Ok(())
    }
}
