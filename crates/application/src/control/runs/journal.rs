//! Durable execution leases, worker receipts and result checkpoints.
use crate::control::LocalControlService;
use crate::control::errors::{error, recovery_error, store_error};
use crate::control::events::pending;
use crate::control::project::git::git_symbolic_head;
use crate::control::project::worktrees::run_workdir;
use crate::control::runs::is_terminal_workspace_status;
use ait_contracts::{AgentMode, ApiError, RunView};
use ait_domain::ErrorCode;
use ait_ports::{ControlStoreError, WorkspaceAgentResponse};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(in crate::control) struct WorkspaceRunJournal {
    #[serde(default)]
    pub(in crate::control) worker_instance_id: Option<String>,
    #[serde(default)]
    pub(in crate::control) worker_receipts: HashMap<String, String>,
    pub(in crate::control) operation_id: String,
    pub(in crate::control) lease_epoch: u64,
    #[serde(default)]
    pub(in crate::control) baseline_ref: Option<String>,
    #[serde(default)]
    pub(in crate::control) result: Option<WorkspaceAgentResponse>,
}

fn workspace_fingerprint(method: &str, value: &impl Serialize) -> Result<String, ApiError> {
    use sha2::Digest;
    let bytes =
        serde_json::to_vec(value).map_err(|_| recovery_error("invalid workspace operation"))?;
    Ok(format!("{method}:{:x}", sha2::Sha256::digest(bytes)))
}

fn record_workspace_receipt(
    journal: &mut WorkspaceRunJournal,
    lease: &WorkspaceExecutionLease,
    operation: Option<&ait_ports::WorkspaceWorkerOperation>,
    fingerprint: &str,
) -> Result<bool, ApiError> {
    let Some(operation) = operation else {
        return Ok(false);
    };
    if operation.lease.run_id.as_str() != lease.run_id
        || operation.lease.epoch != lease.lease_epoch
        || journal.worker_instance_id.as_deref() != Some(&operation.lease.instance_id)
    {
        return Err(recovery_error("stale workspace worker lease"));
    }
    if operation.operation_id.is_empty() || operation.operation_id.len() > 256 {
        return Err(recovery_error("invalid worker operation identity"));
    }
    if let Some(existing) = journal.worker_receipts.get(&operation.operation_id) {
        return if existing == fingerprint {
            Ok(true)
        } else {
            Err(recovery_error("workspace operation identity conflict"))
        };
    }
    if journal.worker_receipts.len() >= 8192 {
        return Err(recovery_error("worker receipt limit exceeded"));
    }
    journal
        .worker_receipts
        .insert(operation.operation_id.clone(), fingerprint.into());
    Ok(false)
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::control) struct WorkspaceExecutionLease {
    pub(in crate::control) run_id: String,
    pub(in crate::control) operation_id: String,
    pub(in crate::control) lease_epoch: u64,
}

pub(in crate::control) fn ensure_current_lease(
    run: &RunView,
    lease: &WorkspaceExecutionLease,
) -> Result<(), ApiError> {
    if run.operation_id.as_deref() != Some(lease.operation_id.as_str())
        || run.lease_epoch != lease.lease_epoch
    {
        return Err(recovery_error("stale workspace execution lease"));
    }
    Ok(())
}

pub(in crate::control) fn ensure_journal_lease(
    journal: &WorkspaceRunJournal,
    lease: &WorkspaceExecutionLease,
) -> Result<(), ApiError> {
    if journal.operation_id != lease.operation_id || journal.lease_epoch != lease.lease_epoch {
        return Err(recovery_error("stale workspace result journal lease"));
    }
    Ok(())
}

impl LocalControlService {
    pub(in crate::control) async fn set_run_running(
        &self,
        run_id: &str,
    ) -> Result<Option<WorkspaceExecutionLease>, ApiError> {
        for _ in 0..4 {
            let loaded = self.read_run_records(run_id).await?;
            let mut state = loaded.original.clone();
            let index = state
                .runs
                .iter()
                .position(|run| run.id == run_id)
                .ok_or_else(|| error(ErrorCode::InvalidRun, "run not found", false))?;
            if state.runs[index].status != "queued" {
                return Ok(None);
            }
            let operation_id = state.runs[index]
                .operation_id
                .as_deref()
                .map_or_else(|| format!("workspace-{run_id}"), str::to_owned);
            let lease_epoch = state.runs[index].lease_epoch.saturating_add(1);
            let baseline_ref = if state.runs[index].provider.kind == AgentMode::Codex {
                git_symbolic_head(&run_workdir(&state, &state.runs[index])?)?
            } else {
                None
            };
            let run = &mut state.runs[index];
            run.status = "running".into();
            run.phase = Some("calling_agent".into());
            run.operation_id = Some(operation_id.clone().into_boxed_str());
            run.lease_epoch = lease_epoch;
            run.error = None;
            state.workspace_run_journals.insert(
                run_id.to_owned(),
                WorkspaceRunJournal {
                    worker_instance_id: None,
                    worker_receipts: HashMap::new(),
                    operation_id: operation_id.clone(),
                    lease_epoch,
                    baseline_ref,
                    result: None,
                },
            );
            let event = pending("run.updated", Some(run_id.to_owned()), run);
            match self.persist_records(&loaded, &state, vec![event]).await {
                Ok(()) => {
                    return Ok(Some(WorkspaceExecutionLease {
                        run_id: run_id.to_owned(),
                        operation_id,
                        lease_epoch,
                    }));
                }
                Err(ControlStoreError::Conflict) => {}
                Err(error) => return Err(store_error(error)),
            }
        }
        Err(error(
            ErrorCode::RunQueueConflict,
            "concurrent run update did not settle",
            true,
        ))
    }

    pub(in crate::control) async fn claim_workspace_worker(
        &self,
        lease: &WorkspaceExecutionLease,
        instance: &str,
    ) -> Result<ait_ports::WorkerLease, ApiError> {
        if uuid::Uuid::parse_str(instance).is_err() {
            return Err(recovery_error("invalid worker identity"));
        }
        for _ in 0..4 {
            let loaded = self.read_run_records(&lease.run_id).await?;
            let mut state = loaded.original.clone();
            let run = state
                .runs
                .iter()
                .find(|r| r.id == lease.run_id)
                .ok_or_else(|| recovery_error("Run missing"))?;
            ensure_current_lease(run, lease)?;
            if is_terminal_workspace_status(&run.status) {
                return Err(recovery_error("terminal Run rejects worker claim"));
            }
            let journal = state
                .workspace_run_journals
                .get_mut(&lease.run_id)
                .ok_or_else(|| recovery_error("workspace journal missing"))?;
            ensure_journal_lease(journal, lease)?;
            if journal
                .worker_instance_id
                .as_ref()
                .is_some_and(|id| id != instance)
            {
                return Err(recovery_error("worker already claimed execution epoch"));
            }
            journal.worker_instance_id = Some(instance.into());
            match self.persist_records(&loaded, &state, Vec::new()).await {
                Ok(()) => {
                    return Ok(ait_ports::WorkerLease {
                        run_id: ait_domain::RunId::new(&lease.run_id),
                        instance_id: instance.into(),
                        epoch: lease.lease_epoch,
                    });
                }
                Err(ControlStoreError::Conflict) => {}
                Err(error) => return Err(store_error(error)),
            }
        }
        Err(recovery_error("worker lease claim conflicted"))
    }

    pub(in crate::control) async fn persist_workspace_result(
        &self,
        lease: &WorkspaceExecutionLease,
        result: WorkspaceAgentResponse,
        worker: Option<&ait_ports::WorkspaceWorkerOperation>,
    ) -> Result<RunView, ApiError> {
        for _ in 0..4 {
            let loaded = self.read_run_records(&lease.run_id).await?;
            let mut state = loaded.original.clone();
            let index = state
                .runs
                .iter()
                .position(|run| run.id == lease.run_id)
                .ok_or_else(|| error(ErrorCode::InvalidRun, "run not found", false))?;
            let mut run = state.runs[index].clone();
            ensure_current_lease(&run, lease)?;
            let journal = state
                .workspace_run_journals
                .get_mut(&lease.run_id)
                .ok_or_else(|| recovery_error("workspace result journal is missing"))?;
            ensure_journal_lease(journal, lease)?;
            let fingerprint = workspace_fingerprint("checkpoint", &result)?;
            if record_workspace_receipt(journal, lease, worker, &fingerprint)? {
                return Ok(run);
            }
            if is_terminal_workspace_status(&run.status) {
                return if journal.result.as_ref() == Some(&result) {
                    Ok(run)
                } else {
                    Err(recovery_error(
                        "terminal Run cannot accept a different workspace result",
                    ))
                };
            }
            let repeated = run.status == "settling" && journal.result.as_ref() == Some(&result);
            if run.status != "running" && !repeated {
                return Err(error(
                    ErrorCode::RunNotResumable,
                    "run is not accepting a workspace result checkpoint",
                    false,
                ));
            }
            journal.result = Some(result.clone());
            run.status = "settling".into();
            run.phase = Some("result_persisted".into());
            run.error = None;
            state.runs[index] = run.clone();
            let events = if repeated {
                Vec::new()
            } else {
                vec![pending(
                    "run.result_persisted",
                    Some(lease.run_id.clone()),
                    &run,
                )]
            };
            match self.persist_records(&loaded, &state, events).await {
                Ok(()) => return Ok(run),
                Err(ControlStoreError::Conflict) => {}
                Err(error) => return Err(store_error(error)),
            }
        }
        Err(error(
            ErrorCode::RunQueueConflict,
            "concurrent workspace result checkpoint did not settle",
            true,
        ))
    }

    pub(in crate::control) async fn claim_workspace_integration(
        &self,
        lease: &WorkspaceExecutionLease,
        worker: Option<&ait_ports::WorkspaceWorkerOperation>,
    ) -> Result<RunView, ApiError> {
        for _ in 0..4 {
            let loaded = self.read_run_records(&lease.run_id).await?;
            let mut state = loaded.original.clone();
            let index = state
                .runs
                .iter()
                .position(|run| run.id == lease.run_id)
                .ok_or_else(|| error(ErrorCode::InvalidRun, "run not found", false))?;
            let mut run = state.runs[index].clone();
            ensure_current_lease(&run, lease)?;
            let journal = state
                .workspace_run_journals
                .get_mut(&lease.run_id)
                .ok_or_else(|| recovery_error("workspace result journal is missing"))?;
            ensure_journal_lease(journal, lease)?;
            if record_workspace_receipt(journal, lease, worker, "integration")? {
                return Ok(run);
            }
            if is_terminal_workspace_status(&run.status) {
                return Err(error(
                    ErrorCode::RunAlreadyTerminal,
                    "workspace Run became terminal before integration",
                    false,
                ));
            }
            if run.status != "settling" || journal.result.is_none() {
                return Err(recovery_error(
                    "workspace integration requires a durable result checkpoint",
                ));
            }
            let repeated = run.phase.as_deref() == Some("integrating");
            run.phase = Some("integrating".into());
            run.error = None;
            state.runs[index] = run.clone();
            let events = if repeated {
                Vec::new()
            } else {
                vec![pending(
                    "run.integration_claimed",
                    Some(lease.run_id.clone()),
                    &run,
                )]
            };
            match self.persist_records(&loaded, &state, events).await {
                Ok(()) => return Ok(run),
                Err(ControlStoreError::Conflict) => {}
                Err(error) => return Err(store_error(error)),
            }
        }
        Err(error(
            ErrorCode::RunQueueConflict,
            "workspace integration claim did not settle",
            true,
        ))
    }

    pub(in crate::control) async fn current_workspace_lease(
        &self,
        run_id: &str,
    ) -> Result<WorkspaceExecutionLease, ApiError> {
        let state = self.read_run_records(run_id).await?.original;
        let run = state
            .runs
            .iter()
            .find(|run| run.id == run_id)
            .ok_or_else(|| error(ErrorCode::InvalidRun, "run not found", false))?;
        let operation_id = run
            .operation_id
            .as_deref()
            .map(str::to_owned)
            .ok_or_else(|| recovery_error("workspace Run has no operation identity"))?;
        Ok(WorkspaceExecutionLease {
            run_id: run_id.to_owned(),
            operation_id,
            lease_epoch: run.lease_epoch,
        })
    }
}
