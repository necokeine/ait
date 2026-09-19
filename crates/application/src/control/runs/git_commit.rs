//! Independent, recoverable Ait Git finalization after authoritative model completion.
use super::RunRecord;
use crate::control::{
    LocalControlService,
    conversation::release_session,
    errors::{error, store_error},
    events::pending,
    project::worktrees::run_workdir,
};
use ait_contracts::{ApiError, RunCommitStatus, RunGitCommit};
use ait_domain::{ErrorCode, LifecyclePhase, LifecycleStatus};
use ait_ports::ControlStoreError;
use ait_workspace::{RunCommitBaseline, RunCommitPlan};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(in crate::control) struct AutoCommit {
    pub view: RunGitCommit,
    pub baseline: Option<RunCommitBaseline>,
    pub plan: Option<RunCommitPlan>,
}

impl AutoCommit {
    pub(in crate::control) fn pending(&self) -> bool {
        matches!(
            self.view.status,
            RunCommitStatus::Pending | RunCommitStatus::Prepared
        )
    }
}

impl LocalControlService {
    pub(in crate::control) async fn capture_auto_commit(
        &self,
        cwd: &Path,
        enabled: bool,
    ) -> Option<AutoCommit> {
        if !enabled {
            return None;
        }
        let result = self.project_workspace.capture_run_commit(cwd).await;
        let (baseline, status, reason) = match result {
            Ok(baseline) => (Some(baseline), RunCommitStatus::Pending, None),
            Err(failure) => (None, RunCommitStatus::Skipped, Some(failure.message)),
        };
        Some(AutoCommit {
            view: RunGitCommit {
                status,
                commit_id: None,
                reason,
            },
            baseline,
            plan: None,
        })
    }

    pub(in crate::control) async fn finalize_native_git(
        &self,
        run: RunRecord,
    ) -> Result<RunRecord, ApiError> {
        if run
            .codex_input
            .as_ref()
            .is_none_or(|input| input.state != super::native::InputState::Published)
            || !matches!(
                run.status(),
                LifecycleStatus::Completed | LifecycleStatus::Settling
            )
            || run
                .auto_commit
                .as_ref()
                .is_none_or(|commit| !commit.pending())
        {
            return Ok(run);
        }
        let state = self.read_run_records(&run.id).await?.original;
        let cwd = run_workdir(&state, &run)?;
        let mut commit = run
            .auto_commit
            .clone()
            .expect("checked enabled auto-commit");
        if commit.plan.is_none() {
            let baseline = commit.baseline.as_ref().ok_or_else(|| {
                error(ErrorCode::InvalidRun, "auto-commit baseline missing", false)
            })?;
            match self
                .project_workspace
                .prepare_run_commit(&cwd, baseline, &run.id)
                .await
            {
                Ok(Some(plan)) => {
                    commit.view.status = RunCommitStatus::Prepared;
                    commit.view.commit_id = Some(plan.commit_id.clone());
                    commit.plan = Some(plan);
                    self.persist_auto_commit(&run.id, commit.clone(), false)
                        .await?;
                }
                Ok(None) => {
                    commit.view.status = RunCommitStatus::Skipped;
                    commit.view.reason = Some("No file changes".into());
                    return self.persist_auto_commit(&run.id, commit, true).await;
                }
                Err(failure) => {
                    commit.view.status = if failure.code == ErrorCode::ProjectGitDirty {
                        RunCommitStatus::Skipped
                    } else {
                        RunCommitStatus::Failed
                    };
                    commit.view.reason = Some(failure.message);
                    return self.persist_auto_commit(&run.id, commit, true).await;
                }
            }
        }
        match self
            .project_workspace
            .publish_run_commit(&cwd, commit.plan.as_ref().expect("prepared commit"))
            .await
        {
            Ok(()) => {
                commit.view.status = RunCommitStatus::Committed;
                commit.view.reason = None;
            }
            Err(failure) => {
                // A prepared receipt survives every ambiguous Git failure. Retrying always
                // reconciles this exact object, never generates another model request.
                commit.view.status = if failure.code == ErrorCode::ProjectGitDirty {
                    RunCommitStatus::Skipped
                } else {
                    RunCommitStatus::Failed
                };
                commit.view.reason = Some(failure.message);
            }
        }
        self.persist_auto_commit(&run.id, commit, true).await
    }

    async fn persist_auto_commit(
        &self,
        run_id: &str,
        commit: AutoCommit,
        terminal: bool,
    ) -> Result<RunRecord, ApiError> {
        for _ in 0..8 {
            let loaded = self.read_run_records(run_id).await?;
            let mut state = loaded.original.clone();
            let index = state
                .runs
                .iter()
                .position(|run| run.id == run_id)
                .ok_or_else(|| error(ErrorCode::InvalidRun, "Run not found", false))?;
            let mut run = state.runs[index].clone();
            run.auto_commit = Some(commit.clone());
            if terminal {
                run.set_status(LifecycleStatus::Completed);
                run.set_phase(Some(LifecyclePhase::Terminal));
                release_session(&mut state, &run);
            } else {
                if let Some(session) = state
                    .sessions
                    .iter_mut()
                    .find(|session| Some(&session.id) == run.session_id.as_ref())
                    && session.active_run_id() != Some(run_id)
                {
                    session
                        .reference
                        .acquire(ait_domain::RunId::new(run_id))
                        .map_err(crate::control::errors::project_error)?;
                }
                run.set_status(LifecycleStatus::Settling);
                run.set_phase(Some(LifecyclePhase::Settling));
            }
            state.runs[index] = run.clone();
            match self
                .persist_records(
                    &loaded,
                    &state,
                    vec![pending("run.updated", Some(run.id.clone()), &run.view())],
                )
                .await
            {
                Ok(()) => return Ok(run),
                Err(ControlStoreError::Conflict) => {}
                Err(failure) => return Err(store_error(failure)),
            }
        }
        Err(error(
            ErrorCode::RunQueueConflict,
            "Git receipt persistence did not settle",
            true,
        ))
    }

    pub(in crate::control) async fn retry_run_commit(
        &self,
        run_id: &str,
    ) -> Result<ait_contracts::CommandResult, ApiError> {
        let _lease = self.acquire_workspace_write_for_run(run_id).await?;
        let state = self.read_run_records(run_id).await?.original;
        let run = state
            .runs
            .iter()
            .find(|run| run.id == run_id)
            .ok_or_else(|| error(ErrorCode::InvalidRun, "Run not found", false))?;
        if state.sessions.iter().any(|session| {
            Some(&session.id) == run.session_id.as_ref() && session.active_run_id().is_some()
        }) {
            return Err(error(
                ErrorCode::SessionBusy,
                "Session is executing another Run",
                false,
            ));
        }
        let mut commit = run
            .auto_commit
            .clone()
            .filter(|commit| commit.view.status == RunCommitStatus::Failed)
            .ok_or_else(|| {
                error(
                    ErrorCode::InvalidRun,
                    "Run has no failed auto-commit",
                    false,
                )
            })?;
        if run.status() != LifecycleStatus::Completed {
            return Err(error(
                ErrorCode::InvalidRun,
                "Codex execution has not completed",
                false,
            ));
        }
        commit.view.status = if commit.plan.is_some() {
            RunCommitStatus::Prepared
        } else {
            RunCommitStatus::Pending
        };
        commit.view.reason = None;
        let admission = self.admission.read().await;
        if self.draining.load(std::sync::atomic::Ordering::Acquire) {
            return Err(error(ErrorCode::RunCancelled, "daemon is draining", false));
        }
        // Explicit retries participate in the daemon's bounded shutdown drain too.
        let _invocation = super::finalization::InvocationGuard::new(
            self.cancellations.clone(),
            run_id,
            tokio_util::sync::CancellationToken::new(),
        );
        let run = self.persist_auto_commit(run_id, commit, false).await?;
        drop(admission);
        self.finalize_native_git(run)
            .await
            .map(|run| ait_contracts::CommandResult::Run(run.view()))
    }
}
