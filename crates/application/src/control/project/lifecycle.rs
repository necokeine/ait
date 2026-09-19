//! Explicit close keeps ownership until application execution has stopped.
use crate::control::{
    LocalControlService,
    errors::{error, store_error},
    persistence::HasRuns,
};
use ait_contracts::{ApiError, Command, CommandResult};
use ait_domain::ErrorCode;

impl LocalControlService {
    pub(in crate::control) async fn bind_project_agent(
        &self,
        project_id: &str,
        source_agent_id: &str,
        agent_id: &str,
    ) -> Result<CommandResult, ApiError> {
        use ait_ports::{ControlFilter, ControlRecordKind as Kind};
        let catalog = self.read_provider_agent_for_binding(agent_id).await?;
        crate::control::catalog::require_named_agent(&catalog.original, agent_id)?;
        let agent = crate::control::catalog::require_agent(&catalog.original, agent_id)?;
        crate::control::catalog::validate_config(&catalog.original, &agent.config)?;
        let read = self
            .store
            .read(&[ControlFilter::id(Kind::Project, project_id)])
            .await
            .map_err(store_error)?;
        if !read.version.compatible_with(&catalog.version) {
            return Err(error(
                ErrorCode::RunQueueConflict,
                "Agent changed while binding; retry",
                true,
            ));
        }
        self.store
            .bind_project_agent(&read.version, project_id, source_agent_id, agent_id)
            .await
            .map_err(store_error)?;
        let record = read
            .records
            .into_iter()
            .find(|record| record.kind == Kind::Project)
            .ok_or_else(|| error(ErrorCode::InvalidProject, "Project not found", false))?;
        let project: super::ProjectRecord = serde_json::from_value(record.value)
            .map_err(crate::control::errors::serialization_error)?;
        Ok(CommandResult::Project(project.view()))
    }

    async fn read_provider_agent_for_binding(
        &self,
        agent_id: &str,
    ) -> Result<
        crate::control::persistence::transaction::RecordTransaction<
            crate::control::catalog::ProviderContext,
        >,
        ApiError,
    > {
        use ait_ports::{ControlFilter, ControlRecordKind as Kind};
        let read = self
            .store
            .read(&[ControlFilter::id(Kind::Agent, agent_id)])
            .await
            .map_err(store_error)?;
        let provider = read
            .records
            .first()
            .and_then(|record| record.value.pointer("/config/provider_id"))
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| error(ErrorCode::AgentNotFound, "Agent not found", false))?;
        self.read_provider_records(provider, true).await
    }
    pub(in crate::control) async fn close_project(
        &self,
        project_id: &str,
    ) -> Result<CommandResult, ApiError> {
        let admission = self.admission.write().await;
        let catalog = self.records().read_project_catalog().await?;
        let mut project = catalog
            .original
            .projects
            .iter()
            .find(|project| project.id == project_id)
            .ok_or_else(|| error(ErrorCode::InvalidProject, "project not found", false))?
            .view();
        if !self
            .store
            .project_is_open(project_id)
            .await
            .map_err(store_error)?
        {
            self.store
                .close_project(project_id)
                .await
                .map_err(store_error)?;
            project.owner = None;
            return Ok(CommandResult::Project(project));
        }
        self.store
            .begin_project_drain(project_id)
            .await
            .map_err(store_error)?;
        drop(admission);
        let records = self.records().read_project_run_records(project_id).await?;
        let ids: Vec<_> = records
            .original
            .runs()
            .iter()
            .map(|run| run.id.clone())
            .collect();
        for run in records.original.runs() {
            if !crate::control::runs::is_terminal_run_status(run.status()) {
                // Box the recursive dispatch. Draining blocks new work while the
                // admission lock remains available to existing Run finalization.
                let response = Box::pin(self.execute(Command::CancelRun {
                    run_id: run.id.clone(),
                }))
                .await;
                if let Some(failure) = response.error {
                    return Err(failure);
                }
            }
        }
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            let active = self
                .cancellations
                .lock()
                .map_err(|_| {
                    error(
                        ErrorCode::RunQueueConflict,
                        "Run drain state unavailable",
                        true,
                    )
                })?
                .keys()
                .any(|id| ids.contains(id));
            if !active {
                break;
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(error(
                    ErrorCode::ProjectWorkspaceBusy,
                    "Project is still draining; retry close after its workers stop",
                    true,
                ));
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
        self.store
            .close_project(project_id)
            .await
            .map_err(store_error)?;
        project.owner = None;
        Ok(CommandResult::Project(project))
    }
}
