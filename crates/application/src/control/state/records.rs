//! Bounded record selection and persistence for each existing command path.
use crate::control::LocalControlService;
use crate::control::errors::{error, store_error};
use crate::control::state::codec::{
    agent_provider_id, decode_records, record_changes, record_value, required_string,
};
use crate::control::state::{LoadedWorkingSet, WorkingSet};
use ait_contracts::{ApiError, Command};
use ait_domain::ErrorCode;
use ait_ports::{ControlFilter, ControlRecordKind, ControlStoreError, PendingEvent};
use serde_json::Value;
use std::collections::HashSet;

impl LocalControlService {
    pub(in crate::control) async fn read_records(
        &self,
        filters: Vec<ControlFilter>,
    ) -> Result<LoadedWorkingSet, ApiError> {
        decode_records(self.store.read(&filters).await.map_err(store_error)?)
    }

    pub(in crate::control) async fn persist_records(
        &self,
        loaded: &LoadedWorkingSet,
        updated: &WorkingSet,
        events: Vec<PendingEvent>,
    ) -> Result<(), ControlStoreError> {
        self.store
            .apply(
                loaded.revision,
                record_changes(&loaded.original, updated)
                    .map_err(|failure| ControlStoreError::Other(failure.message))?,
                events,
            )
            .await
            .map(|_| ())
    }

    pub(in crate::control) async fn read_provider_records(
        &self,
        provider_id: &str,
        include_agents: bool,
    ) -> Result<LoadedWorkingSet, ApiError> {
        use ControlRecordKind as Kind;
        let mut filters = vec![
            ControlFilter::id(Kind::Provider, provider_id),
            ControlFilter::id(Kind::ProviderCredential, provider_id),
        ];
        if include_agents {
            filters.push(ControlFilter::agents_for_provider(provider_id));
        }
        self.read_records(filters).await
    }

    async fn read_session_records_with(
        &self,
        session_id: &str,
        extra: Vec<ControlFilter>,
    ) -> Result<LoadedWorkingSet, ApiError> {
        use ControlRecordKind as Kind;
        for _ in 0..4 {
            let session_read = self
                .store
                .read(&[ControlFilter::id(Kind::Session, session_id)])
                .await
                .map_err(store_error)?;
            let session = record_value(&session_read, Kind::Session, session_id)
                .ok_or_else(|| error(ErrorCode::SessionNotFound, "session not found", false))?;
            let project_id = required_string(session, "project_id")?;
            let agent_id = required_string(session, "agent_id")?;
            let message_id = required_string(session, "current_message_id")?;
            let agent_read = self
                .store
                .read(&[ControlFilter::id(Kind::Agent, &agent_id)])
                .await
                .map_err(store_error)?;
            if agent_read.revision != session_read.revision {
                continue;
            }
            let agent = record_value(&agent_read, Kind::Agent, &agent_id).ok_or_else(|| {
                error(
                    ErrorCode::InvalidAgentConfiguration,
                    "agent not found",
                    false,
                )
            })?;
            let provider_id = agent_provider_id(agent)?;
            let mut filters = vec![
                ControlFilter::id(Kind::Session, session_id),
                ControlFilter::id(Kind::Project, project_id),
                ControlFilter::id(Kind::Agent, agent_id),
                ControlFilter::id(Kind::Provider, &provider_id),
                ControlFilter::id(Kind::ProviderCredential, provider_id),
                ControlFilter::id(Kind::Message, message_id),
                ControlFilter::all(Kind::Settings),
            ];
            filters.extend(extra.iter().cloned());
            let loaded = self.read_records(filters).await?;
            if loaded.revision == session_read.revision {
                return Ok(loaded);
            }
        }
        Err(error(
            ErrorCode::RunQueueConflict,
            "concurrent Session references did not settle",
            true,
        ))
    }

    pub(in crate::control) async fn read_session_records(
        &self,
        session_id: &str,
    ) -> Result<LoadedWorkingSet, ApiError> {
        self.read_session_records_with(session_id, Vec::new()).await
    }

    pub(in crate::control) async fn read_session_title_records(
        &self,
        session_id: &str,
    ) -> Result<LoadedWorkingSet, ApiError> {
        self.read_session_records_with(
            session_id,
            vec![ControlFilter::runs_for_session(session_id)],
        )
        .await
    }

    pub(in crate::control) async fn read_run_records(
        &self,
        run_id: &str,
    ) -> Result<LoadedWorkingSet, ApiError> {
        use ControlRecordKind as Kind;
        for _ in 0..4 {
            let run_read = self
                .store
                .read(&[ControlFilter::id(Kind::Run, run_id)])
                .await
                .map_err(store_error)?;
            let run = record_value(&run_read, Kind::Run, run_id)
                .ok_or_else(|| error(ErrorCode::InvalidRun, "run not found", false))?;
            let project_id = required_string(run, "project_id")?;
            let message_id = run
                .get("last_message_id")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .unwrap_or(required_string(run, "base_message_id")?);
            let mut filters = vec![
                ControlFilter::id(Kind::Run, run_id),
                ControlFilter::id(Kind::Project, project_id),
                ControlFilter::id(Kind::RunCredential, run_id),
                ControlFilter::id(Kind::WorkspaceRunJournal, run_id),
                ControlFilter::message_ancestors(message_id),
                ControlFilter::all(Kind::Settings),
            ];
            if let Some(session_id) = run.get("session_id").and_then(Value::as_str) {
                filters.push(ControlFilter::id(Kind::Session, session_id));
            }
            if run.get("config").is_none() {
                filters.push(ControlFilter::id(
                    Kind::Agent,
                    required_string(run, "agent_id")?,
                ));
            }
            let loaded = self.read_records(filters).await?;
            if loaded.revision == run_read.revision {
                return Ok(loaded);
            }
        }
        Err(error(
            ErrorCode::RunQueueConflict,
            "concurrent Run references did not settle",
            true,
        ))
    }

    async fn read_run_control_records(&self, run_id: &str) -> Result<LoadedWorkingSet, ApiError> {
        use ControlRecordKind as Kind;
        for _ in 0..4 {
            let run_read = self
                .store
                .read(&[ControlFilter::id(Kind::Run, run_id)])
                .await
                .map_err(store_error)?;
            let run = record_value(&run_read, Kind::Run, run_id)
                .ok_or_else(|| error(ErrorCode::InvalidRun, "run not found", false))?;
            let mut filters = vec![
                ControlFilter::id(Kind::Run, run_id),
                ControlFilter::id(Kind::Project, required_string(run, "project_id")?),
                ControlFilter::id(Kind::WorkspaceRunJournal, run_id),
            ];
            if let Some(session_id) = run.get("session_id").and_then(Value::as_str) {
                filters.push(ControlFilter::id(Kind::Session, session_id));
            }
            if run.get("config").is_none() {
                filters.push(ControlFilter::id(
                    Kind::Agent,
                    required_string(run, "agent_id")?,
                ));
            }
            let loaded = self.read_records(filters).await?;
            if loaded.revision == run_read.revision {
                return Ok(loaded);
            }
        }
        Err(error(
            ErrorCode::RunQueueConflict,
            "concurrent Run control references did not settle",
            true,
        ))
    }

    async fn read_project_run_records(
        &self,
        project_id: &str,
    ) -> Result<LoadedWorkingSet, ApiError> {
        use ControlRecordKind as Kind;
        for _ in 0..4 {
            let runs = self
                .store
                .read(&[ControlFilter::project(Kind::Run, project_id)])
                .await
                .map_err(store_error)?;
            let mut filters = vec![ControlFilter::project(Kind::Run, project_id)];
            filters.extend(
                runs.records
                    .iter()
                    .filter(|record| record.value.get("config").is_none())
                    .filter_map(|record| record.value.get("agent_id").and_then(Value::as_str))
                    .map(|agent_id| ControlFilter::id(Kind::Agent, agent_id)),
            );
            let loaded = self.read_records(filters).await?;
            if loaded.revision == runs.revision {
                return Ok(loaded);
            }
        }
        Err(error(
            ErrorCode::RunQueueConflict,
            "concurrent Project Run index did not settle",
            true,
        ))
    }

    async fn read_cron_records(&self, cron_id: &str) -> Result<LoadedWorkingSet, ApiError> {
        use ControlRecordKind as Kind;
        for _ in 0..4 {
            let cron_read = self
                .store
                .read(&[ControlFilter::id(Kind::Cron, cron_id)])
                .await
                .map_err(store_error)?;
            let cron = record_value(&cron_read, Kind::Cron, cron_id)
                .ok_or_else(|| error(ErrorCode::InvalidCron, "cron not found", false))?;
            let project_id = required_string(cron, "project_id")?;
            let agent_id = required_string(cron, "agent_id")?;
            let message_id = required_string(cron, "base_message_id")?;
            let agent_read = self
                .store
                .read(&[ControlFilter::id(Kind::Agent, &agent_id)])
                .await
                .map_err(store_error)?;
            if agent_read.revision != cron_read.revision {
                continue;
            }
            let agent = record_value(&agent_read, Kind::Agent, &agent_id).ok_or_else(|| {
                error(
                    ErrorCode::InvalidAgentConfiguration,
                    "agent not found",
                    false,
                )
            })?;
            let provider_id = agent_provider_id(agent)?;
            let loaded = self
                .read_records(vec![
                    ControlFilter::id(Kind::Cron, cron_id),
                    ControlFilter::id(Kind::Project, project_id),
                    ControlFilter::id(Kind::Message, message_id),
                    ControlFilter::id(Kind::Agent, agent_id),
                    ControlFilter::id(Kind::Provider, &provider_id),
                    ControlFilter::id(Kind::ProviderCredential, provider_id),
                    ControlFilter::runs_for_cron(cron_id),
                    ControlFilter::all(Kind::Settings),
                ])
                .await?;
            if loaded.revision == cron_read.revision {
                return Ok(loaded);
            }
        }
        Err(error(
            ErrorCode::RunQueueConflict,
            "concurrent Cron references did not settle",
            true,
        ))
    }

    async fn read_new_session_records(
        &self,
        session_id: &str,
        project_id: &str,
        agent_id: &str,
        at_message_id: Option<&str>,
        include_credential: bool,
    ) -> Result<LoadedWorkingSet, ApiError> {
        use ControlRecordKind as Kind;
        for _ in 0..4 {
            let anchors = self
                .store
                .read(&[
                    ControlFilter::id(Kind::Project, project_id),
                    ControlFilter::id(Kind::Agent, agent_id),
                ])
                .await
                .map_err(store_error)?;
            let project = record_value(&anchors, Kind::Project, project_id)
                .ok_or_else(|| error(ErrorCode::InvalidProject, "project not found", false))?;
            let agent = record_value(&anchors, Kind::Agent, agent_id).ok_or_else(|| {
                error(
                    ErrorCode::InvalidAgentConfiguration,
                    "agent not found",
                    false,
                )
            })?;
            let message_id = at_message_id.map_or_else(
                || required_string(project, "root_message_id"),
                |id| Ok(id.to_owned()),
            )?;
            let provider_id = agent_provider_id(agent)?;
            let mut filters = vec![
                ControlFilter::id(Kind::Project, project_id),
                ControlFilter::id(Kind::Agent, agent_id),
                ControlFilter::id(Kind::Provider, &provider_id),
                ControlFilter::id(Kind::Session, session_id),
                ControlFilter::id(Kind::Message, message_id),
                ControlFilter::project(Kind::Message, project_id),
                ControlFilter::all(Kind::Settings),
            ];
            if include_credential {
                filters.push(ControlFilter::id(Kind::ProviderCredential, provider_id));
            }
            let loaded = self.read_records(filters).await?;
            if loaded.revision == anchors.revision {
                return Ok(loaded);
            }
        }
        Err(error(
            ErrorCode::RunQueueConflict,
            "concurrent Session creation references did not settle",
            true,
        ))
    }

    async fn read_derive_session_records(
        &self,
        session_id: &str,
        project_id: &str,
        source_session_id: &str,
        agent_id: &str,
        at_message_id: &str,
    ) -> Result<LoadedWorkingSet, ApiError> {
        use ControlRecordKind as Kind;
        for _ in 0..4 {
            let anchors = self
                .store
                .read(&[
                    ControlFilter::id(Kind::Project, project_id),
                    ControlFilter::id(Kind::Session, source_session_id),
                    ControlFilter::id(Kind::Agent, agent_id),
                ])
                .await
                .map_err(store_error)?;
            record_value(&anchors, Kind::Project, project_id)
                .ok_or_else(|| error(ErrorCode::InvalidProject, "project not found", false))?;
            let source_session = record_value(&anchors, Kind::Session, source_session_id)
                .ok_or_else(|| error(ErrorCode::SessionNotFound, "session not found", false))?;
            let source_agent_id = required_string(source_session, "agent_id")?;
            let requested_agent =
                record_value(&anchors, Kind::Agent, agent_id).ok_or_else(|| {
                    error(
                        ErrorCode::InvalidAgentConfiguration,
                        "agent not found",
                        false,
                    )
                })?;

            let agent_records = self
                .store
                .read(&[
                    ControlFilter::id(Kind::Agent, &source_agent_id),
                    ControlFilter::id(Kind::Agent, agent_id),
                ])
                .await
                .map_err(store_error)?;
            if agent_records.revision != anchors.revision {
                continue;
            }
            let source_agent = record_value(&agent_records, Kind::Agent, &source_agent_id)
                .ok_or_else(|| {
                    error(
                        ErrorCode::InvalidAgentConfiguration,
                        "Session Agent not found",
                        false,
                    )
                })?;
            let provider_ids = [
                agent_provider_id(requested_agent)?,
                agent_provider_id(source_agent)?,
            ];
            let mut filters = vec![
                ControlFilter::id(Kind::Project, project_id),
                ControlFilter::id(Kind::Session, source_session_id),
                ControlFilter::id(Kind::Session, session_id),
                ControlFilter::id(Kind::Message, at_message_id),
                ControlFilter::project(Kind::Message, project_id),
                ControlFilter::message_children(at_message_id),
                ControlFilter::id(Kind::Agent, agent_id),
                ControlFilter::id(Kind::Agent, source_agent_id),
                ControlFilter::all(Kind::Settings),
            ];
            for provider_id in provider_ids {
                filters.push(ControlFilter::id(Kind::Provider, &provider_id));
                filters.push(ControlFilter::id(Kind::ProviderCredential, provider_id));
            }
            let loaded = self.read_records(filters).await?;
            if loaded.revision == anchors.revision {
                return Ok(loaded);
            }
        }
        Err(error(
            ErrorCode::RunQueueConflict,
            "concurrent Session derivation references did not settle",
            true,
        ))
    }

    async fn read_export_records(&self, project_id: &str) -> Result<LoadedWorkingSet, ApiError> {
        use ControlRecordKind as Kind;
        for _ in 0..4 {
            let project_records = self
                .read_records(vec![
                    ControlFilter::id(Kind::Project, project_id),
                    ControlFilter::project(Kind::Session, project_id),
                    ControlFilter::project(Kind::Message, project_id),
                ])
                .await?;
            let project = project_records
                .original
                .projects
                .iter()
                .find(|project| project.id == project_id)
                .ok_or_else(|| error(ErrorCode::InvalidProject, "project not found", false))?;
            let mut agent_ids = project_records
                .original
                .sessions
                .iter()
                .map(|session| session.agent_id.clone())
                .collect::<HashSet<_>>();
            if let Some(agent_id) = &project.default_agent_id {
                agent_ids.insert(agent_id.clone());
            }
            let mut filters = vec![
                ControlFilter::id(Kind::Project, project_id),
                ControlFilter::project(Kind::Session, project_id),
                ControlFilter::project(Kind::Message, project_id),
            ];
            filters.extend(
                agent_ids
                    .iter()
                    .map(|agent_id| ControlFilter::id(Kind::Agent, agent_id)),
            );
            let agent_records = self.store.read(&filters).await.map_err(store_error)?;
            if agent_records.revision != project_records.revision {
                continue;
            }
            let mut provider_ids = HashSet::new();
            for agent_id in &agent_ids {
                let agent =
                    record_value(&agent_records, Kind::Agent, agent_id).ok_or_else(|| {
                        error(
                            ErrorCode::InvalidAgentConfiguration,
                            "export Agent is unavailable",
                            false,
                        )
                    })?;
                provider_ids.insert(agent_provider_id(agent)?);
            }
            filters.extend(
                provider_ids
                    .iter()
                    .map(|provider_id| ControlFilter::id(Kind::Provider, provider_id)),
            );
            let loaded = self.read_records(filters).await?;
            if loaded.revision == project_records.revision {
                return Ok(loaded);
            }
        }
        Err(error(
            ErrorCode::RunQueueConflict,
            "concurrent Project export references did not settle",
            true,
        ))
    }

    #[allow(clippy::too_many_lines)]
    pub(in crate::control) async fn read_command_records(
        &self,
        command: &Command,
    ) -> Result<LoadedWorkingSet, ApiError> {
        use ControlRecordKind as Kind;
        match command {
            Command::RegisterProject { .. } | Command::ListProjects => {
                self.read_records(vec![ControlFilter::all(Kind::Project)])
                    .await
            }
            Command::RegisterAgent { id, config, .. } | Command::UpdateAgent { id, config, .. } => {
                self.read_records(vec![
                    ControlFilter::id(Kind::Agent, id),
                    ControlFilter::id(Kind::Provider, &config.provider_id),
                ])
                .await
            }
            Command::SaveAgentProvider { provider, .. } => {
                self.read_provider_records(&provider.id, true).await
            }
            Command::DiscoverProviderModels { provider, .. } => {
                self.read_provider_records(&provider.id, false).await
            }
            Command::RefreshProviderModels { provider_id } => {
                self.read_provider_records(provider_id, false).await
            }
            Command::ListAgents => {
                self.read_records(vec![ControlFilter::all(Kind::Agent)])
                    .await
            }
            Command::ListAgentProviders => {
                self.read_records(vec![
                    ControlFilter::all(Kind::Provider),
                    ControlFilter::all(Kind::ProviderCredential),
                ])
                .await
            }
            Command::SetProjectDefaultAgent {
                project_id,
                agent_id,
            } => {
                self.read_records(vec![
                    ControlFilter::id(Kind::Project, project_id),
                    ControlFilter::id(Kind::Agent, agent_id),
                ])
                .await
            }
            Command::CreateCron {
                id,
                project_id,
                base_message_id,
                agent_id,
                ..
            } => {
                self.read_records(vec![
                    ControlFilter::id(Kind::Cron, id),
                    ControlFilter::id(Kind::Project, project_id),
                    ControlFilter::id(Kind::Message, base_message_id),
                    ControlFilter::id(Kind::Agent, agent_id),
                ])
                .await
            }
            Command::ExportProject { project_id } => self.read_export_records(project_id).await,
            Command::ListMessages { project_id } => {
                self.read_records(vec![ControlFilter::project(Kind::Message, project_id)])
                    .await
            }
            Command::ListRuns { project_id } => self.read_project_run_records(project_id).await,
            Command::CreateSession {
                id,
                project_id,
                agent_id,
                at_message_id,
                ..
            } => {
                self.read_new_session_records(
                    id,
                    project_id,
                    agent_id,
                    at_message_id.as_deref(),
                    false,
                )
                .await
            }
            Command::ForkSession {
                id,
                project_id,
                agent_id,
                at_message_id,
                ..
            } => {
                self.read_new_session_records(id, project_id, agent_id, Some(at_message_id), true)
                    .await
            }
            Command::DeriveSession {
                id,
                project_id,
                source_session_id,
                agent_id,
                at_message_id,
                ..
            } => {
                self.read_derive_session_records(
                    id,
                    project_id,
                    source_session_id,
                    agent_id,
                    at_message_id,
                )
                .await
            }
            Command::SetSessionConfig { session_id, config } => {
                self.read_session_records_with(
                    session_id,
                    vec![ControlFilter::id(Kind::Provider, &config.provider_id)],
                )
                .await
            }
            Command::SetSessionAgent {
                session_id,
                agent_id,
            } => {
                self.read_session_records_with(
                    session_id,
                    vec![ControlFilter::id(Kind::Agent, agent_id)],
                )
                .await
            }
            Command::RenameSession { session_id, .. }
            | Command::SetSessionTitle { session_id, .. }
            | Command::SendMessage { session_id, .. } => {
                self.read_session_records(session_id).await
            }
            Command::GetRun { run_id }
            | Command::CancelRun { run_id }
            | Command::ResolveNativeApproval { run_id, .. } => {
                self.read_run_control_records(run_id).await
            }
            Command::SetCronEnabled { cron_id, .. } | Command::TriggerCron { cron_id, .. } => {
                self.read_cron_records(cron_id).await
            }
            Command::GetSettings | Command::SaveSettings { .. } | Command::ResetSettings => {
                self.read_records(vec![ControlFilter::all(Kind::Settings)])
                    .await
            }
            Command::ListSessions { project_id } => {
                self.read_records(vec![ControlFilter::project(Kind::Session, project_id)])
                    .await
            }
            Command::ListCrons => {
                self.read_records(vec![ControlFilter::all(Kind::Cron)])
                    .await
            }
            Command::ImportProject { archive, .. } => {
                let mut filters = vec![ControlFilter::all(Kind::Project)];
                filters.extend(
                    archive
                        .agents
                        .iter()
                        .map(|agent| ControlFilter::id(Kind::Agent, &agent.id)),
                );
                filters.extend(
                    archive
                        .providers
                        .iter()
                        .map(|provider| ControlFilter::id(Kind::Provider, &provider.id)),
                );
                filters.extend(
                    archive
                        .messages
                        .iter()
                        .map(|message| ControlFilter::id(Kind::Message, &message.id)),
                );
                filters.extend(
                    archive
                        .sessions
                        .iter()
                        .map(|session| ControlFilter::id(Kind::Session, &session.id)),
                );
                self.read_records(filters).await
            }
        }
    }
}
