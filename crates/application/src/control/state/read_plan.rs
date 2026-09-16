//! Bounded record selection and persistence for each existing command path.
use crate::control::errors::{error, store_error};
use crate::control::state::codec::{
    agent_provider_id, decode_records, record_value, required_string,
};
use crate::control::state::commands::CommandTransaction;
use crate::control::state::records::RecordAccess;
use crate::control::state::transaction::{RecordContext, RecordTransaction};
use crate::control::state::{
    ApiRunContext, ArchiveContext, ConversationContext, CronTriggerContext, ProviderContext,
    RunContext, RunControlContext, RunsContext, SessionTitleContext,
};
use ait_contracts::{ApiError, Command};
use ait_domain::ErrorCode;
use ait_ports::{ControlFilter, ControlRecordKind, ControlStoreError, PendingEvent};
use serde_json::Value;
use std::collections::HashSet;

impl RecordAccess {
    pub(in crate::control) async fn read_run_view_records(
        &self,
        run_id: &str,
    ) -> Result<RecordTransaction<RunsContext>, ApiError> {
        self.read_records(vec![ControlFilter::id(ControlRecordKind::Run, run_id)])
            .await
    }
    pub(in crate::control) async fn read_message_path_records(
        &self,
        head_id: &str,
    ) -> Result<RecordTransaction<crate::control::state::MessagesContext>, ApiError> {
        self.read_records(vec![ControlFilter::message_ancestors(head_id)])
            .await
    }
    pub(in crate::control) async fn read_session_record(
        &self,
        session_id: &str,
    ) -> Result<RecordTransaction<crate::control::state::SessionsContext>, ApiError> {
        self.read_records(vec![ControlFilter::id(
            ControlRecordKind::Session,
            session_id,
        )])
        .await
    }
    pub(in crate::control) async fn read_project_catalog(
        &self,
    ) -> Result<RecordTransaction<crate::control::state::ProjectsContext>, ApiError> {
        self.read_records(vec![ControlFilter::all(ControlRecordKind::Project)])
            .await
    }

    async fn read_session_config_records<C: RecordContext>(
        &self,
        session_id: &str,
        provider_id: Option<&str>,
        agent_id: Option<&str>,
    ) -> Result<RecordTransaction<C>, ApiError> {
        use ControlRecordKind as Kind;
        for _ in 0..4 {
            let anchor = self
                .store
                .read(&[ControlFilter::id(Kind::Session, session_id)])
                .await
                .map_err(store_error)?;
            let session = record_value(&anchor, Kind::Session, session_id)
                .ok_or_else(|| error(ErrorCode::SessionNotFound, "session not found", false))?;
            let selected_agent = agent_id.map_or_else(
                || required_string(session, "agent_id"),
                |id| Ok(id.to_owned()),
            )?;
            let mut filters = vec![
                ControlFilter::id(Kind::Session, session_id),
                ControlFilter::id(Kind::Agent, selected_agent),
            ];
            if let Some(id) = provider_id {
                filters.push(ControlFilter::id(Kind::Provider, id));
            }
            let read = self.store.read(&filters).await.map_err(store_error)?;
            if read.revision == anchor.revision {
                return decode_records(&read);
            }
        }
        Err(error(
            ErrorCode::RunQueueConflict,
            "concurrent Session binding references did not settle",
            true,
        ))
    }
    pub(in crate::control) async fn read_records<C: RecordContext>(
        &self,
        filters: Vec<ControlFilter>,
    ) -> Result<RecordTransaction<C>, ApiError> {
        decode_records(&self.store.read(&filters).await.map_err(store_error)?)
    }

    pub(in crate::control) async fn persist_records<C: RecordContext>(
        &self,
        loaded: &RecordTransaction<C>,
        updated: &C,
        events: Vec<PendingEvent>,
    ) -> Result<(), ControlStoreError> {
        loaded.commit(self.store.as_ref(), updated, events).await
    }

    pub(in crate::control) async fn read_provider_records(
        &self,
        provider_id: &str,
        include_agents: bool,
    ) -> Result<RecordTransaction<ProviderContext>, ApiError> {
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
    ) -> Result<RecordTransaction<ConversationContext>, ApiError> {
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
                ControlFilter::id(Kind::Settings, "settings"),
            ];
            filters.extend(extra.iter().cloned());
            let read = self.store.read(&filters).await.map_err(store_error)?;
            if read.revision == session_read.revision {
                return decode_records(&read);
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
    ) -> Result<RecordTransaction<ConversationContext>, ApiError> {
        self.read_session_records_with(session_id, Vec::new()).await
    }

    pub(in crate::control) async fn read_session_title_records(
        &self,
        session_id: &str,
    ) -> Result<RecordTransaction<SessionTitleContext>, ApiError> {
        use ControlRecordKind as Kind;
        for _ in 0..4 {
            let anchor = self
                .store
                .read(&[ControlFilter::id(Kind::Session, session_id)])
                .await
                .map_err(store_error)?;
            let session = record_value(&anchor, Kind::Session, session_id)
                .ok_or_else(|| error(ErrorCode::SessionNotFound, "session not found", false))?;
            let read = self
                .store
                .read(&[
                    ControlFilter::id(Kind::Session, session_id),
                    ControlFilter::id(Kind::Project, required_string(session, "project_id")?),
                    ControlFilter::id(
                        Kind::Message,
                        required_string(session, "current_message_id")?,
                    ),
                    ControlFilter::runs_for_session(session_id),
                ])
                .await
                .map_err(store_error)?;
            if read.revision == anchor.revision {
                return decode_records(&read);
            }
        }
        Err(error(
            ErrorCode::RunQueueConflict,
            "concurrent Session title references did not settle",
            true,
        ))
    }

    pub(in crate::control) async fn read_run_records(
        &self,
        run_id: &str,
    ) -> Result<RecordTransaction<RunContext>, ApiError> {
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
                ControlFilter::id(Kind::Settings, "settings"),
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
            let read = self.store.read(&filters).await.map_err(store_error)?;
            if read.revision == run_read.revision {
                return decode_records(&read);
            }
        }
        Err(error(
            ErrorCode::RunQueueConflict,
            "concurrent Run references did not settle",
            true,
        ))
    }

    pub(in crate::control) async fn read_api_run_records(
        &self,
        run_id: &str,
    ) -> Result<RecordTransaction<ApiRunContext>, ApiError> {
        use ControlRecordKind as Kind;
        for _ in 0..4 {
            let run_read = self
                .store
                .read(&[ControlFilter::id(Kind::Run, run_id)])
                .await
                .map_err(store_error)?;
            let run = record_value(&run_read, Kind::Run, run_id)
                .ok_or_else(|| error(ErrorCode::InvalidRun, "run not found", false))?;
            let message_id = run
                .get("last_message_id")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .unwrap_or(required_string(run, "base_message_id")?);
            let mut filters = vec![
                ControlFilter::id(Kind::Run, run_id),
                ControlFilter::message_ancestors(message_id),
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
            let read = self.store.read(&filters).await.map_err(store_error)?;
            if read.revision == run_read.revision {
                return decode_records(&read);
            }
        }
        Err(error(
            ErrorCode::RunQueueConflict,
            "concurrent Run references did not settle",
            true,
        ))
    }

    async fn read_run_control_records(
        &self,
        run_id: &str,
    ) -> Result<RecordTransaction<RunControlContext>, ApiError> {
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
            let read = self.store.read(&filters).await.map_err(store_error)?;
            if read.revision == run_read.revision {
                return decode_records(&read);
            }
        }
        Err(error(
            ErrorCode::RunQueueConflict,
            "concurrent Run control references did not settle",
            true,
        ))
    }

    pub(in crate::control) async fn read_project_run_records(
        &self,
        project_id: &str,
    ) -> Result<RecordTransaction<RunsContext>, ApiError> {
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
            let read = self.store.read(&filters).await.map_err(store_error)?;
            if read.revision == runs.revision {
                return decode_records(&read);
            }
        }
        Err(error(
            ErrorCode::RunQueueConflict,
            "concurrent Project Run index did not settle",
            true,
        ))
    }

    async fn read_cron_records(
        &self,
        cron_id: &str,
    ) -> Result<RecordTransaction<CronTriggerContext>, ApiError> {
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
            let read = self
                .store
                .read(&[
                    ControlFilter::id(Kind::Cron, cron_id),
                    ControlFilter::id(Kind::Project, project_id),
                    ControlFilter::id(Kind::Message, message_id),
                    ControlFilter::id(Kind::Agent, agent_id),
                    ControlFilter::id(Kind::Provider, &provider_id),
                    ControlFilter::id(Kind::ProviderCredential, provider_id),
                    ControlFilter::runs_for_cron(cron_id),
                    ControlFilter::id(Kind::Settings, "settings"),
                ])
                .await
                .map_err(store_error)?;
            if read.revision == cron_read.revision {
                return decode_records(&read);
            }
        }
        Err(error(
            ErrorCode::RunQueueConflict,
            "concurrent Cron references did not settle",
            true,
        ))
    }

    async fn read_new_session_records<C: RecordContext>(
        &self,
        session_id: &str,
        project_id: &str,
        agent_id: &str,
        at_message_id: Option<&str>,
        include_credential: bool,
    ) -> Result<RecordTransaction<C>, ApiError> {
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

            let mut filters = vec![
                ControlFilter::id(Kind::Project, project_id),
                ControlFilter::id(Kind::Agent, agent_id),
                ControlFilter::id(Kind::Session, session_id),
                ControlFilter::message_ancestors(message_id),
            ];
            if include_credential {
                let provider_id = agent_provider_id(agent)?;
                filters.push(ControlFilter::id(Kind::Provider, &provider_id));
                filters.push(ControlFilter::id(Kind::Settings, "settings"));
                filters.push(ControlFilter::id(Kind::ProviderCredential, provider_id));
            }
            let read = self.store.read(&filters).await.map_err(store_error)?;
            if read.revision == anchors.revision {
                return decode_records(&read);
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
    ) -> Result<RecordTransaction<ConversationContext>, ApiError> {
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
                ControlFilter::message_ancestors(at_message_id),
                ControlFilter::message_children(at_message_id),
                ControlFilter::id(Kind::Agent, agent_id),
                ControlFilter::id(Kind::Agent, source_agent_id),
                ControlFilter::id(Kind::Settings, "settings"),
            ];
            for provider_id in provider_ids {
                filters.push(ControlFilter::id(Kind::Provider, &provider_id));
                filters.push(ControlFilter::id(Kind::ProviderCredential, provider_id));
            }
            let read = self.store.read(&filters).await.map_err(store_error)?;
            if read.revision == anchors.revision {
                return decode_records(&read);
            }
        }
        Err(error(
            ErrorCode::RunQueueConflict,
            "concurrent Session derivation references did not settle",
            true,
        ))
    }

    async fn read_export_records(
        &self,
        project_id: &str,
    ) -> Result<RecordTransaction<ArchiveContext>, ApiError> {
        use ControlRecordKind as Kind;
        for _ in 0..4 {
            let project_records = self
                .read_records::<ArchiveContext>(vec![
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
                .map(|session| session.agent_id().to_owned())
                .collect::<HashSet<_>>();
            if let Some(agent_id) = &project.default_agent_id() {
                agent_ids.insert(agent_id.to_string());
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
            let read = self.store.read(&filters).await.map_err(store_error)?;
            if read.revision == project_records.revision {
                return decode_records(&read);
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
    ) -> Result<CommandTransaction, ApiError> {
        use ControlRecordKind as Kind;
        match command {
            Command::RegisterProject { id, workdir, .. } => ({
                self.read_records(project_identity_plan(id, workdir.as_deref()))
                    .await
            })
            .map(CommandTransaction::ProjectRegistration),
            Command::ListProjects => ({
                self.read_records(vec![ControlFilter::all(Kind::Project)])
                    .await
            })
            .map(CommandTransaction::Projects),
            Command::RegisterAgent { id, config, .. } | Command::UpdateAgent { id, config, .. } => {
                ({
                    self.read_records(vec![
                        ControlFilter::id(Kind::Agent, id),
                        ControlFilter::id(Kind::Provider, &config.provider_id),
                    ])
                    .await
                })
                .map(CommandTransaction::Agent)
            }
            Command::SaveAgentProvider { provider, .. } => {
                ({ self.read_provider_records(&provider.id, true).await })
                    .map(CommandTransaction::Provider)
            }
            Command::DiscoverProviderModels { provider, .. } => {
                ({ self.read_provider_records(&provider.id, false).await })
                    .map(CommandTransaction::Provider)
            }
            Command::RefreshProviderModels { provider_id } => {
                ({ self.read_provider_records(provider_id, false).await })
                    .map(CommandTransaction::Provider)
            }
            Command::ListAgents => ({
                self.read_records(vec![ControlFilter::all(Kind::Agent)])
                    .await
            })
            .map(CommandTransaction::Agents),
            Command::ListAgentProviders => ({
                self.read_records(vec![
                    ControlFilter::all(Kind::Provider),
                    ControlFilter::all(Kind::ProviderCredential),
                ])
                .await
            })
            .map(CommandTransaction::Provider),
            Command::UpdateProject {
                project_id,
                agent_id,
                ..
            } => {
                let mut filters = vec![ControlFilter::id(Kind::Project, project_id)];
                if let Some(agent_id) = agent_id {
                    filters.push(ControlFilter::id(Kind::Agent, agent_id));
                }
                self.read_records(filters)
                    .await
                    .map(CommandTransaction::ProjectAgent)
            }
            Command::SetProjectDefaultAgent {
                project_id,
                agent_id,
            } => ({
                self.read_records(vec![
                    ControlFilter::id(Kind::Project, project_id),
                    ControlFilter::id(Kind::Agent, agent_id),
                ])
                .await
            })
            .map(CommandTransaction::ProjectAgent),
            Command::CreateCron {
                id,
                project_id: _,
                base_message_id,
                agent_id,
                ..
            } => ({
                self.read_records(vec![
                    ControlFilter::id(Kind::Cron, id),
                    ControlFilter::id(Kind::Message, base_message_id),
                    ControlFilter::id(Kind::Agent, agent_id),
                ])
                .await
            })
            .map(CommandTransaction::CronCreate),
            Command::ExportProject { project_id } => {
                (self.read_export_records(project_id).await).map(CommandTransaction::Archive)
            }
            Command::ListMessages { project_id } => ({
                self.read_records(vec![ControlFilter::project(Kind::Message, project_id)])
                    .await
            })
            .map(CommandTransaction::Messages),
            Command::ListRuns { project_id } => {
                (self.read_project_run_records(project_id).await).map(CommandTransaction::Runs)
            }
            Command::CreateSession {
                id,
                project_id,
                agent_id,
                at_message_id,
                ..
            } => ({
                self.read_new_session_records(
                    id,
                    project_id,
                    agent_id,
                    at_message_id.as_deref(),
                    false,
                )
                .await
            })
            .map(CommandTransaction::NewSession),
            Command::ForkSession {
                id,
                project_id,
                agent_id,
                at_message_id,
                ..
            } => ({
                self.read_new_session_records(id, project_id, agent_id, Some(at_message_id), true)
                    .await
            })
            .map(CommandTransaction::Conversation),
            Command::DeriveSession {
                id,
                project_id,
                source_session_id,
                agent_id,
                at_message_id,
                ..
            } => ({
                self.read_derive_session_records(
                    id,
                    project_id,
                    source_session_id,
                    agent_id,
                    at_message_id,
                )
                .await
            })
            .map(CommandTransaction::Conversation),
            Command::SetSessionConfig { session_id, config } => ({
                self.read_session_config_records(session_id, Some(&config.provider_id), None)
                    .await
            })
            .map(CommandTransaction::SessionConfig),
            Command::SetSessionAgent {
                session_id,
                agent_id,
            } => ({
                self.read_session_config_records(session_id, None, Some(agent_id))
                    .await
            })
            .map(CommandTransaction::SessionBinding),
            Command::RenameSession { session_id, .. }
            | Command::SetSessionTitle { session_id, .. } => ({
                self.read_records(vec![ControlFilter::id(Kind::Session, session_id)])
                    .await
            })
            .map(CommandTransaction::Sessions),
            Command::SendMessage { session_id, .. } => {
                ({ self.read_session_records(session_id).await })
                    .map(CommandTransaction::Conversation)
            }
            Command::GetRun { run_id } => self
                .read_run_view_records(run_id)
                .await
                .map(CommandTransaction::Runs),
            Command::CancelRun { run_id } | Command::ResolveNativeApproval { run_id, .. } => {
                ({ self.read_run_control_records(run_id).await })
                    .map(CommandTransaction::RunControl)
            }
            Command::SetCronEnabled { cron_id, .. } => ({
                self.read_records(vec![ControlFilter::id(Kind::Cron, cron_id)])
                    .await
            })
            .map(CommandTransaction::Crons),
            Command::TriggerCron { cron_id, .. } => {
                ({ self.read_cron_records(cron_id).await }).map(CommandTransaction::CronTrigger)
            }
            Command::GetSettings | Command::SaveSettings { .. } | Command::ResetSettings => ({
                self.read_records(vec![ControlFilter::id(Kind::Settings, "settings")])
                    .await
            })
            .map(CommandTransaction::Settings),
            Command::ListSessions { project_id } => ({
                self.read_records(vec![ControlFilter::project(Kind::Session, project_id)])
                    .await
            })
            .map(CommandTransaction::Sessions),
            Command::ListCrons => ({
                self.read_records(vec![ControlFilter::all(Kind::Cron)])
                    .await
            })
            .map(CommandTransaction::Crons),
            Command::ImportProject { archive, workdir } => ({
                let mut filters = project_identity_plan(&archive.project.id, Some(workdir));
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
            })
            .map(CommandTransaction::Archive),
        }
    }
}

fn project_identity_plan(id: &str, workdir: Option<&str>) -> Vec<ControlFilter> {
    let mut filters = vec![ControlFilter::id(ControlRecordKind::Project, id)];
    // The preparation phase supplies the canonical path before any commit.
    if let Some(workdir) = workdir {
        filters.push(ControlFilter::ProjectWorkdir {
            workdir: workdir.to_owned(),
        });
    }
    filters
}
