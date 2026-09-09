#![allow(missing_docs)]

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fmt::Write as _,
    fs::{File, OpenOptions},
    panic::AssertUnwindSafe,
    path::{Path, PathBuf},
    process::Command as ProcessCommand,
    sync::{
        Arc, Mutex, OnceLock, Weak,
        atomic::{AtomicBool, Ordering},
    },
    time::{SystemTime, UNIX_EPOCH},
};

use ait_contracts::{
    API_VERSION, AgentConfiguration, AgentMode, AgentProvider, AgentProviderView, AgentView,
    ApiError, Command, CommandResult, CronView, Event, EventPage, MessageView,
    NativeApprovalAction, NativeApprovalView, NativePermissionProfile, PROJECT_EXPORT_VERSION,
    ProjectExport, ProjectView, ProtocolRequestId, ProviderModel, Response, RunView, SessionView,
    SettingKind, SettingsDocument, SettingsView, default_settings, settings_schema,
};
use ait_domain::{
    AgentId, ApprovalGrantScope, ApprovalMode, Cron, CronConcurrencyPolicy, CronId,
    CronMisfirePolicy, DomainError, ErrorCode, MessageId, NativeApprovalKind, NativeApprovalStatus,
    NativeApprovalTarget, ProjectId, RunPermissionProfile, SandboxAccess, TimestampMs,
};
use ait_ports::{
    AgentProviderGateway, ControlChange, ControlFilter, ControlRead, ControlRecord,
    ControlRecordKind, ControlStore, ControlStoreError, HostProviderModelCatalog, PendingEvent,
    ProviderMessage, SessionTitleGenerator, SessionTitleRequest, WorkspaceAgent,
    WorkspaceAgentInvocation, WorkspaceAgentResponse, WorkspaceApproval, WorkspaceApprovalDecision,
    WorkspaceApprovalRequest, WorkspaceIntegrationGate, WorkspaceOutputItem, WorkspaceResultSink,
};
use futures_util::FutureExt;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use uuid::Uuid;

mod agents;
mod progress;
use agents::{
    InvocationGuard, agent_for_session, builtin_providers, check_session_admission, migrate_state,
    register_agent, require_named_agent, set_session_config, update_agent, validate_config,
    validate_provider,
};
use progress::ProgressPump;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct WorkingSet {
    #[serde(default)]
    projects: Vec<ProjectView>,
    #[serde(default)]
    agents: Vec<AgentView>,
    #[serde(default)]
    providers: Vec<AgentProviderView>,
    #[serde(default)]
    provider_credentials: HashMap<String, String>,
    #[serde(default)]
    run_credentials: HashMap<String, String>,
    #[serde(default)]
    sessions: Vec<SessionView>,
    #[serde(default)]
    messages: Vec<MessageView>,
    #[serde(default)]
    runs: Vec<RunView>,
    /// Durable completed Agent responses keyed by Run id. The production Codex
    /// adapter writes this checkpoint before publishing its isolated commit.
    #[serde(default)]
    workspace_run_journals: HashMap<String, WorkspaceRunJournal>,
    #[serde(default)]
    crons: Vec<CronView>,
    #[serde(default = "default_settings")]
    settings: SettingsDocument,
    #[serde(default = "default_settings_revision")]
    settings_revision: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct WorkspaceRunJournal {
    operation_id: String,
    lease_epoch: u64,
    #[serde(default)]
    baseline_ref: Option<String>,
    #[serde(default)]
    result: Option<WorkspaceAgentResponse>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct WorkspaceExecutionLease {
    run_id: String,
    operation_id: String,
    lease_epoch: u64,
}

struct DurableWorkspaceResultSink {
    service: LocalControlService,
    lease: WorkspaceExecutionLease,
    checkpointed: AtomicBool,
}

impl DurableWorkspaceResultSink {
    fn is_checkpointed(&self) -> bool {
        self.checkpointed.load(Ordering::Acquire)
    }
}

#[async_trait::async_trait]
impl WorkspaceResultSink for DurableWorkspaceResultSink {
    async fn checkpoint(&self, result: WorkspaceAgentResponse) -> Result<(), DomainError> {
        self.service
            .persist_workspace_result(&self.lease, result)
            .await
            .map_err(api_domain_error)?;
        self.checkpointed.store(true, Ordering::Release);
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RecoveryPolicy {
    ResumeSafe,
    Ask,
    Fail,
}

#[derive(Clone, Debug, PartialEq)]
enum WorkspaceRecoveryClaim {
    Execute,
    Finalize(WorkspaceExecutionLease),
    Recovered(Box<RunView>),
    Skip,
}

/// Read-only startup scan result. Run ownership changes only after the
/// supervisor acquires the matching Project workspace lease.
pub struct StartupRecoveryPlan {
    run_ids: Vec<String>,
}

impl StartupRecoveryPlan {
    #[must_use]
    pub fn len(&self) -> usize {
        self.run_ids.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.run_ids.is_empty()
    }
}

struct ForkSessionInput {
    id: String,
    project_id: String,
    agent_id: String,
    at_message_id: String,
    text: String,
}

/// Internal continuation produced by a command, consumed only after its state
/// is committed. A public `RunView` is a result snapshot, never an execution signal.
enum CommandOutcome {
    Ready(Box<CommandResult>),
    ExecuteWorkspaceRun(Box<RunView>),
}

/// Owns both the async in-process queue position and the process-wide advisory
/// lock for one canonical Project Git worktree.
struct WorkspaceWriteLease {
    _process_guard: tokio::sync::OwnedMutexGuard<()>,
    _file: File,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct GitBaseline {
    commit: String,
    index_tree: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum WorkspaceFinalizationDecision {
    Open,
    Integrating,
    Cancelled,
}

struct WorkspaceRunControl {
    cancellation: tokio_util::sync::CancellationToken,
    finalization: tokio::sync::Mutex<WorkspaceFinalizationDecision>,
    integration_lease: OnceLock<WorkspaceIntegrationLease>,
}

#[derive(Clone)]
struct WorkspaceIntegrationLease {
    service: LocalControlService,
    lease: WorkspaceExecutionLease,
}

impl std::fmt::Debug for WorkspaceRunControl {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("WorkspaceRunControl")
            .field("cancellation", &self.cancellation)
            .field("finalization", &self.finalization)
            .field(
                "integration_lease_bound",
                &self.integration_lease.get().is_some(),
            )
            .finish()
    }
}

impl WorkspaceRunControl {
    fn new() -> Self {
        Self {
            cancellation: tokio_util::sync::CancellationToken::new(),
            finalization: tokio::sync::Mutex::new(WorkspaceFinalizationDecision::Open),
            integration_lease: OnceLock::new(),
        }
    }

    fn bind_integration_lease(
        &self,
        service: LocalControlService,
        lease: WorkspaceExecutionLease,
    ) -> Result<(), ApiError> {
        self.integration_lease
            .set(WorkspaceIntegrationLease { service, lease })
            .map_err(|_| recovery_error("workspace integration lease was bound more than once"))
    }
}

#[async_trait::async_trait]
impl WorkspaceIntegrationGate for WorkspaceRunControl {
    async fn begin_integration(&self) -> Result<(), DomainError> {
        let mut decision = self.finalization.lock().await;
        match *decision {
            WorkspaceFinalizationDecision::Open if !self.cancellation.is_cancelled() => {}
            WorkspaceFinalizationDecision::Integrating => return Ok(()),
            WorkspaceFinalizationDecision::Open | WorkspaceFinalizationDecision::Cancelled => {
                return Err(DomainError::invariant(
                    ErrorCode::RunCancelled,
                    "run cancellation won before workspace integration",
                ));
            }
        }
        let binding = self.integration_lease.get().ok_or_else(|| {
            DomainError::invariant(
                ErrorCode::RunRecoveryFailed,
                "workspace integration has no durable execution lease",
            )
        })?;
        binding
            .service
            .claim_workspace_integration(&binding.lease)
            .await
            .map_err(api_domain_error)?;
        *decision = WorkspaceFinalizationDecision::Integrating;
        Ok(())
    }
}

struct WorkspaceRunControlGuard {
    controls: Arc<Mutex<HashMap<String, Weak<WorkspaceRunControl>>>>,
    id: String,
}

impl WorkspaceRunControlGuard {
    fn new(
        controls: Arc<Mutex<HashMap<String, Weak<WorkspaceRunControl>>>>,
        id: &str,
        control: &Arc<WorkspaceRunControl>,
    ) -> Self {
        controls
            .lock()
            .expect("workspace run controls")
            .insert(id.to_owned(), Arc::downgrade(control));
        Self {
            controls,
            id: id.to_owned(),
        }
    }
}

impl Drop for WorkspaceRunControlGuard {
    fn drop(&mut self) {
        self.controls
            .lock()
            .expect("workspace run controls")
            .remove(&self.id);
    }
}

impl CommandOutcome {
    fn for_new_run(run: RunView) -> Self {
        Self::ExecuteWorkspaceRun(Box::new(run))
    }
}

impl Default for WorkingSet {
    fn default() -> Self {
        Self {
            projects: Vec::new(),
            agents: Vec::new(),
            providers: builtin_providers(),
            provider_credentials: HashMap::new(),
            run_credentials: HashMap::new(),
            sessions: Vec::new(),
            messages: Vec::new(),
            runs: Vec::new(),
            workspace_run_journals: HashMap::new(),
            crons: Vec::new(),
            settings: default_settings(),
            settings_revision: default_settings_revision(),
        }
    }
}

const fn default_settings_revision() -> u64 {
    1
}

struct LoadedWorkingSet {
    revision: u64,
    original: WorkingSet,
}

/// Shared application entry point used by every transport adapter.
#[derive(Clone)]
pub struct LocalControlService {
    store: Arc<dyn ControlStore>,
    session_leases: Arc<Mutex<HashMap<String, Weak<()>>>>,
    workspace_leases: Arc<Mutex<HashMap<PathBuf, Weak<tokio::sync::Mutex<()>>>>>,
    cancellations: Arc<Mutex<HashMap<String, tokio_util::sync::CancellationToken>>>,
    workspace_run_controls: Arc<Mutex<HashMap<String, Weak<WorkspaceRunControl>>>>,
    approval_waiters:
        Arc<Mutex<HashMap<String, tokio::sync::watch::Sender<Option<WorkspaceApprovalDecision>>>>>,
    permission_limits: PermissionPolicyLimits,
    provider_gateway: Option<Arc<dyn AgentProviderGateway>>,
    host_provider_catalog: Option<Arc<dyn HostProviderModelCatalog>>,
    workspace_agent: Option<Arc<dyn WorkspaceAgent>>,
    session_title_generator: Option<Arc<dyn SessionTitleGenerator>>,
}

/// Administrator-owned ceiling applied before a Run or approval can have side effects.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PermissionPolicyLimits {
    pub max_sandbox: SandboxAccess,
    pub allow_session_approvals: bool,
}

impl Default for PermissionPolicyLimits {
    fn default() -> Self {
        Self {
            max_sandbox: SandboxAccess::FullAccess,
            allow_session_approvals: true,
        }
    }
}

impl LocalControlService {
    #[must_use]
    pub fn new(store: Arc<dyn ControlStore>) -> Self {
        Self {
            store,
            session_leases: Arc::new(Mutex::new(HashMap::new())),
            workspace_leases: Arc::new(Mutex::new(HashMap::new())),
            cancellations: Arc::new(Mutex::new(HashMap::new())),
            workspace_run_controls: Arc::new(Mutex::new(HashMap::new())),
            approval_waiters: Arc::new(Mutex::new(HashMap::new())),
            permission_limits: PermissionPolicyLimits::default(),
            provider_gateway: None,
            host_provider_catalog: None,
            workspace_agent: None,
            session_title_generator: None,
        }
    }

    /// Creates a service that can execute real workspace-scoped coding Agents.
    #[must_use]
    pub fn with_workspace_agent(
        store: Arc<dyn ControlStore>,
        workspace_agent: Arc<dyn WorkspaceAgent>,
    ) -> Self {
        Self {
            store,
            session_leases: Arc::new(Mutex::new(HashMap::new())),
            workspace_leases: Arc::new(Mutex::new(HashMap::new())),
            cancellations: Arc::new(Mutex::new(HashMap::new())),
            workspace_run_controls: Arc::new(Mutex::new(HashMap::new())),
            approval_waiters: Arc::new(Mutex::new(HashMap::new())),
            permission_limits: PermissionPolicyLimits::default(),
            provider_gateway: None,
            host_provider_catalog: None,
            workspace_agent: Some(workspace_agent),
            session_title_generator: None,
        }
    }

    #[must_use]
    pub fn with_provider_gateway(mut self, gateway: Arc<dyn AgentProviderGateway>) -> Self {
        self.provider_gateway = Some(gateway);
        self
    }

    /// Applies an administrator-owned upper bound to subsequent Run admission.
    #[must_use]
    pub fn with_permission_limits(mut self, limits: PermissionPolicyLimits) -> Self {
        self.permission_limits = limits;
        self
    }

    /// Adds model discovery for host-authenticated providers such as Codex.
    #[must_use]
    pub fn with_host_provider_catalog(
        mut self,
        catalog: Arc<dyn HostProviderModelCatalog>,
    ) -> Self {
        self.host_provider_catalog = Some(catalog);
        self
    }

    /// Adds the read-only generator used for first-interaction Session metadata.
    #[must_use]
    pub fn with_session_title_generator(
        mut self,
        generator: Arc<dyn SessionTitleGenerator>,
    ) -> Self {
        self.session_title_generator = Some(generator);
        self
    }

    async fn read_records(
        &self,
        filters: Vec<ControlFilter>,
    ) -> Result<LoadedWorkingSet, ApiError> {
        decode_records(self.store.read(&filters).await.map_err(store_error)?)
    }

    async fn persist_records(
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

    async fn read_provider_records(
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

    async fn read_session_records(&self, session_id: &str) -> Result<LoadedWorkingSet, ApiError> {
        self.read_session_records_with(session_id, Vec::new()).await
    }

    async fn read_session_title_records(
        &self,
        session_id: &str,
    ) -> Result<LoadedWorkingSet, ApiError> {
        self.read_session_records_with(
            session_id,
            vec![ControlFilter::runs_for_session(session_id)],
        )
        .await
    }

    async fn read_run_records(&self, run_id: &str) -> Result<LoadedWorkingSet, ApiError> {
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
            let message_id = required_string(run, "base_message_id")?;
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
                ControlFilter::message_children(at_message_id),
                ControlFilter::id(Kind::Agent, agent_id),
                ControlFilter::id(Kind::Agent, source_agent_id),
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
    async fn read_command_records(&self, command: &Command) -> Result<LoadedWorkingSet, ApiError> {
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
                self.read_records(vec![project_id.as_ref().map_or_else(
                    || ControlFilter::all(Kind::Session),
                    |id| ControlFilter::project(Kind::Session, id),
                )])
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

    /// Executes one versioned command and returns a stable response envelope.
    pub async fn execute(&self, command: Command) -> Response {
        match self.try_execute(command).await {
            Ok(result) => Response::success(result),
            Err(error) => Response::failure(error),
        }
    }

    /// Persists a new interactive Run and returns immediately while the
    /// daemon-owned task continues independently of the requesting client.
    pub async fn submit(self: &Arc<Self>, command: Command) -> Response {
        match self.try_submit(command).await {
            Ok(result) => Response::success(result),
            Err(error) => Response::failure(error),
        }
    }

    async fn try_submit(self: &Arc<Self>, command: Command) -> Result<CommandResult, ApiError> {
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

    /// Runs the one-shot background title turn after a Session's first interaction.
    pub async fn generate_session_title(
        &self,
        session_id: String,
        user_prompt: String,
    ) -> Response {
        match self
            .try_generate_session_title(&session_id, &user_prompt)
            .await
        {
            Ok(session) => Response::success(CommandResult::Session(session)),
            Err(error) => Response::failure(error),
        }
    }

    async fn try_generate_session_title(
        &self,
        session_id: &str,
        user_prompt: &str,
    ) -> Result<SessionView, ApiError> {
        let bounded_prompt = user_prompt.chars().take(2_000).collect::<String>();
        if bounded_prompt.trim().is_empty() {
            return Err(error(
                ErrorCode::InvalidSession,
                "Session title prompt is empty",
                false,
            ));
        }
        let (session, workdir, should_generate, local_only) =
            self.begin_title_generation(session_id).await?;
        if !should_generate || local_only {
            return Ok(session);
        }
        let generator = self.session_title_generator.as_ref().ok_or_else(|| {
            error(
                ErrorCode::InvalidConfiguration,
                "Session title generator is not configured",
                false,
            )
        })?;
        let generated = generator
            .generate(SessionTitleRequest {
                request_id: format!("session-title-{}", Uuid::new_v4()),
                user_prompt: bounded_prompt,
                cwd: workdir.into(),
                cancellation: tokio_util::sync::CancellationToken::new(),
            })
            .await
            .map_err(|failure| error(failure.code, failure.message, failure.retryable))?;
        validate_session_metadata(&generated.title, &generated.description)?;
        self.finish_title_generation(session_id, generated.title, generated.description)
            .await
    }

    async fn begin_title_generation(
        &self,
        session_id: &str,
    ) -> Result<(SessionView, String, bool, bool), ApiError> {
        for _ in 0..4 {
            let loaded = self.read_session_title_records(session_id).await?;
            let mut state = loaded.original.clone();
            let index = state
                .sessions
                .iter()
                .position(|session| session.id == session_id)
                .ok_or_else(|| error(ErrorCode::SessionNotFound, "session not found", false))?;
            let session = state.sessions[index].clone();
            let project = state
                .projects
                .iter()
                .find(|project| project.id == session.project_id)
                .ok_or_else(|| error(ErrorCode::InvalidProject, "project not found", false))?;
            #[cfg(all(feature = "dev-mock-provider", debug_assertions))]
            let local_only = state.runs.iter().any(|run| {
                run.session_id.as_deref() == Some(session_id)
                    && run.provider.kind == AgentMode::Mock
            });
            #[cfg(not(all(feature = "dev-mock-provider", debug_assertions)))]
            let local_only = false;
            if session.title_generation_started || !session.name.trim().is_empty() {
                return Ok((session, project.workdir.clone(), false, local_only));
            }
            if !is_first_completed_interaction(&state, &session) {
                return Err(error(
                    ErrorCode::InvalidSession,
                    "Session has not completed its first interaction",
                    false,
                ));
            }
            state.sessions[index].title_generation_started = true;
            let session = state.sessions[index].clone();
            let event = pending(
                "session.title_generation_started",
                Some(session_id.to_owned()),
                &session,
            );
            match self.persist_records(&loaded, &state, vec![event]).await {
                Ok(()) => {
                    return Ok((session, project.workdir.clone(), true, local_only));
                }
                Err(ControlStoreError::Conflict) => {}
                Err(error) => return Err(store_error(error)),
            }
        }
        Err(error(
            ErrorCode::RunQueueConflict,
            "concurrent Session title update did not settle",
            true,
        ))
    }

    async fn finish_title_generation(
        &self,
        session_id: &str,
        title: String,
        description: String,
    ) -> Result<SessionView, ApiError> {
        for _ in 0..4 {
            let loaded = self.read_session_records(session_id).await?;
            let mut state = loaded.original.clone();
            let session = state
                .sessions
                .iter_mut()
                .find(|session| session.id == session_id)
                .ok_or_else(|| error(ErrorCode::SessionNotFound, "session not found", false))?;
            session.title = Some(title.clone());
            session.description.clone_from(&description);
            let session = session.clone();
            let event = pending(
                "session.title_generated",
                Some(session_id.to_owned()),
                &session,
            );
            match self.persist_records(&loaded, &state, vec![event]).await {
                Ok(()) => return Ok(session),
                Err(ControlStoreError::Conflict) => {}
                Err(error) => return Err(store_error(error)),
            }
        }
        Err(error(
            ErrorCode::RunQueueConflict,
            "concurrent Session title update did not settle",
            true,
        ))
    }

    async fn execute_workspace_agent(
        &self,
        run_id: &str,
        control: Arc<WorkspaceRunControl>,
    ) -> Result<RunView, ApiError> {
        let loaded = self.read_run_records(run_id).await?;
        let state = loaded.original;
        let run = state
            .runs
            .iter()
            .find(|run| run.id == run_id)
            .ok_or_else(|| error(ErrorCode::InvalidRun, "run not found", false))?;
        let run = run.clone();
        let Some(lease) = self.set_run_running(&run.id).await? else {
            let state = self.read_run_records(&run.id).await?.original;
            return state
                .runs
                .into_iter()
                .find(|candidate| candidate.id == run.id)
                .ok_or_else(|| error(ErrorCode::InvalidRun, "run not found", false));
        };
        control.bind_integration_lease(self.clone(), lease.clone())?;
        let progress = ProgressPump::start(self.store.clone(), &run);
        let reporter = progress.reporter();
        let cancellation = control.cancellation.clone();
        let result_sink = DurableWorkspaceResultSink {
            service: self.clone(),
            lease: lease.clone(),
            checkpointed: AtomicBool::new(false),
        };
        let call = async {
            match run.provider.kind {
                AgentMode::OpenAI | AgentMode::DeepSeek => self.invoke_provider(&state, &run).await,
                AgentMode::Codex => {
                    self.invoke_codex_workspace_checkpointed(
                        &state,
                        &run,
                        control.clone(),
                        reporter.clone(),
                        &result_sink,
                    )
                    .await
                }
                #[cfg(all(feature = "dev-mock-provider", debug_assertions))]
                AgentMode::Mock => Ok(Self::invoke_mock()),
            }
        };
        let result = AssertUnwindSafe(async {
            if run.provider.kind == AgentMode::Codex {
                // Workspace execution owns its complete settlement path. The
                // supervisor containing this call survives transport cancellation.
                call.await
            } else {
                tokio::pin!(call);
                tokio::select! {
                    biased;
                    () = cancellation.cancelled() => {
                        Err(DomainError::invariant(ErrorCode::RunCancelled, "run was cancelled"))
                    },
                    result = &mut call => result,
                }
            }
        })
        .catch_unwind()
        .await;
        drop(reporter);
        // Progress storage is deliberately best-effort: a display-channel
        // failure must not prevent Git integration or terminal persistence.
        // This drain also runs after a provider panic, before any terminal
        // commit can clear the checkpoint.
        let _ = progress.finish().await;
        let result = result.unwrap_or_else(|_| {
            Err(DomainError::invariant(
                ErrorCode::ProviderFailed,
                "workspace agent task panicked",
            ))
        });
        if result.is_ok() && run.provider.kind != AgentMode::Codex {
            // Settling is an informational state for the live UI. Once the
            // workspace result exists, failure to expose that intermediate
            // state must not bypass the reliable terminal persistence path.
            let _ = self.set_run_settling(&lease).await;
        }
        if run.provider.kind == AgentMode::Codex && result.is_ok() && !result_sink.is_checkpointed()
        {
            return self
                .finish_workspace_run(
                    &lease,
                    Err(DomainError::invariant(
                        ErrorCode::RunRecoveryFailed,
                        "workspace adapter returned without durably checkpointing its result",
                    )),
                )
                .await;
        }
        self.finish_workspace_run(&lease, result).await
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "the invocation keeps immutable admission, finalization, and progress context together"
    )]
    async fn invoke_codex_workspace_checkpointed(
        &self,
        state: &WorkingSet,
        run: &RunView,
        control: Arc<WorkspaceRunControl>,
        progress: Arc<dyn ait_ports::WorkspaceProgressReporter>,
        result_sink: &dyn WorkspaceResultSink,
    ) -> Result<WorkspaceAgentResponse, DomainError> {
        let executor = self.workspace_agent.as_ref().ok_or_else(|| {
            DomainError::invariant(
                ErrorCode::InvalidConfiguration,
                "Codex workspace executor is not configured",
            )
        })?;
        let invocation = workspace_invocation(state, run, control, Arc::new(self.clone()))?;
        executor
            .invoke_with_progress_and_checkpoint(invocation, progress, result_sink)
            .await
    }

    async fn supervise_workspace_agent(
        &self,
        run_id: String,
        control: Arc<WorkspaceRunControl>,
    ) -> Result<RunView, ApiError> {
        let worker = self.clone();
        let worker_run_id = run_id.clone();
        let task_control = Arc::clone(&control);
        let task = tokio::spawn(async move {
            worker
                .execute_workspace_agent(&worker_run_id, task_control)
                .await
        });
        let result = match task.await {
            Ok(Ok(run)) => Ok(run),
            Ok(Err(failure)) => {
                // Admission already made this Run visible to an asynchronous
                // caller. Every later error therefore belongs to the Run and
                // must be persisted before its Session and workspace leases
                // are released.
                self.finish_current_workspace_run(&run_id, Err(api_domain_error(failure)))
                    .await
            }
            Err(failure) => {
                self.finish_current_workspace_run(
                    &run_id,
                    Err(DomainError::invariant(
                        ErrorCode::ProviderFailed,
                        format!("workspace execution task failed: {failure}"),
                    )),
                )
                .await
            }
        };
        drop(control);
        result
    }

    async fn set_run_running(
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
                let project = state
                    .projects
                    .iter()
                    .find(|project| project.id == state.runs[index].project_id)
                    .ok_or_else(|| error(ErrorCode::InvalidProject, "project not found", false))?;
                git_symbolic_head(Path::new(&project.workdir))?
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

    async fn persist_workspace_result(
        &self,
        lease: &WorkspaceExecutionLease,
        result: WorkspaceAgentResponse,
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
            if is_terminal_workspace_status(&run.status) {
                return if journal.result.as_ref() == Some(&result) {
                    Ok(run)
                } else {
                    Err(recovery_error(
                        "terminal Run cannot accept a different workspace result",
                    ))
                };
            }
            if run.status == "settling" && journal.result.as_ref() == Some(&result) {
                return Ok(run);
            }
            if run.status != "running" {
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
            let event = pending("run.result_persisted", Some(lease.run_id.clone()), &run);
            match self.persist_records(&loaded, &state, vec![event]).await {
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

    async fn claim_workspace_integration(
        &self,
        lease: &WorkspaceExecutionLease,
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
                .get(&lease.run_id)
                .ok_or_else(|| recovery_error("workspace result journal is missing"))?;
            ensure_journal_lease(journal, lease)?;
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
            if run.phase.as_deref() == Some("integrating") {
                return Ok(run);
            }
            run.phase = Some("integrating".into());
            run.error = None;
            state.runs[index] = run.clone();
            let event = pending("run.integration_claimed", Some(lease.run_id.clone()), &run);
            match self.persist_records(&loaded, &state, vec![event]).await {
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

    async fn current_workspace_lease(
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

    async fn finish_current_workspace_run(
        &self,
        run_id: &str,
        result: Result<WorkspaceAgentResponse, DomainError>,
    ) -> Result<RunView, ApiError> {
        let lease = self.current_workspace_lease(run_id).await?;
        self.finish_workspace_run(&lease, result).await
    }

    async fn set_run_settling(&self, lease: &WorkspaceExecutionLease) -> Result<bool, ApiError> {
        for _ in 0..4 {
            let loaded = self.read_run_records(&lease.run_id).await?;
            let mut state = loaded.original.clone();
            let run = state
                .runs
                .iter_mut()
                .find(|run| run.id == lease.run_id)
                .ok_or_else(|| error(ErrorCode::InvalidRun, "run not found", false))?;
            ensure_current_lease(run, lease)?;
            if run.status != "running" {
                return Ok(false);
            }
            run.status = "settling".into();
            run.phase = Some("settling".into());
            let event = pending("run.updated", Some(lease.run_id.clone()), run);
            match self.persist_records(&loaded, &state, vec![event]).await {
                Ok(()) => return Ok(true),
                Err(ControlStoreError::Conflict) => {}
                Err(error) => return Err(store_error(error)),
            }
        }
        Err(error(
            ErrorCode::RunQueueConflict,
            "concurrent run settling update did not settle",
            true,
        ))
    }

    #[allow(
        clippy::too_many_lines,
        reason = "the terminal snapshot and immutable Message must stay visibly atomic"
    )]
    async fn finish_workspace_run(
        &self,
        lease: &WorkspaceExecutionLease,
        result: Result<WorkspaceAgentResponse, DomainError>,
    ) -> Result<RunView, ApiError> {
        let mut persistence_failures = 0_u32;
        loop {
            let Ok(loaded) = self.read_run_records(&lease.run_id).await else {
                wait_for_workspace_terminal_persistence(&mut persistence_failures).await;
                continue;
            };
            let mut state = loaded.original.clone();
            let index = state
                .runs
                .iter()
                .position(|run| run.id == lease.run_id)
                .ok_or_else(|| error(ErrorCode::InvalidRun, "run not found", false))?;
            let mut run = state.runs[index].clone();
            ensure_current_lease(&run, lease)?;
            if let Some(journal) = state.workspace_run_journals.get(&lease.run_id) {
                ensure_journal_lease(journal, lease)?;
                if let Ok(output) = &result
                    && journal.result.as_ref().is_some_and(|saved| saved != output)
                {
                    return Err(recovery_error(
                        "terminal settlement received a different workspace result",
                    ));
                }
            }
            if is_terminal_workspace_status(&run.status) {
                let _ = self.store.clear_progress(&lease.run_id).await;
                return Ok(run);
            }
            if run.status == "settling" && result.is_err() {
                let failure = result.as_ref().expect_err("checked error");
                run.status = "interrupted".into();
                run.error = Some(error(
                    failure.code,
                    format!(
                        "checkpointed workspace result could not be integrated: {}",
                        failure.message
                    ),
                    failure.retryable,
                ));
            } else {
                apply_workspace_terminal_result(&mut state, &mut run, &result);
            }
            let approval_status = if result
                .as_ref()
                .is_err_and(|failure| failure.code == ErrorCode::RunCancelled)
            {
                NativeApprovalStatus::Cancelled
            } else {
                NativeApprovalStatus::Expired
            };
            expire_pending_native_approvals(&mut run, approval_status);
            run.phase = Some("terminal".into());
            release_session(&mut state, &run);
            state.runs[index] = run.clone();
            let event = pending("run.updated", Some(lease.run_id.clone()), &run);
            match self.persist_records(&loaded, &state, vec![event]).await {
                Ok(()) => {
                    self.notify_approval_waiters(&run);
                    let _ = self.store.clear_progress(&lease.run_id).await;
                    return Ok(run);
                }
                Err(ControlStoreError::Conflict | ControlStoreError::Other(_)) => {
                    // A workspace adapter may already have made its Git result
                    // externally visible. Keep the supervisor's finalization
                    // control and Project/Session leases until the matching
                    // terminal Run, assistant output, and commit audit are all
                    // durable. Returning here would reopen cancellation and
                    // workspace admission around an unaudited integration.
                    wait_for_workspace_terminal_persistence(&mut persistence_failures).await;
                }
            }
        }
    }

    /// Replays durable events after a cursor, allowing lossless reconnection.
    ///
    /// # Errors
    ///
    /// Returns a stable recovery error when persistence cannot replay the outbox.
    pub async fn replay_events(&self, after: u64, limit: usize) -> Result<Vec<Event>, ApiError> {
        self.store
            .replay(after, limit.clamp(1, 1_000))
            .await
            .map_err(store_error)
            .map(|events| {
                events
                    .into_iter()
                    .map(|event| Event {
                        api_version: API_VERSION,
                        cursor: event.cursor,
                        kind: event.kind,
                        entity_id: event.entity_id,
                        body: event.body,
                        created_at: event.created_at,
                    })
                    .collect()
            })
    }

    /// Replays a page and reports whether a nonzero reconnect cursor is still
    /// inside the retained event window.
    ///
    /// # Errors
    ///
    /// Returns a stable persistence error when bounds or events cannot be read.
    pub async fn event_page(&self, after: u64, limit: usize) -> Result<EventPage, ApiError> {
        let page = self
            .store
            .replay_page(after, limit.clamp(1, 1_000))
            .await
            .map_err(store_error)?;
        Ok(EventPage {
            events: page
                .events
                .into_iter()
                .map(|event| Event {
                    api_version: API_VERSION,
                    cursor: event.cursor,
                    kind: event.kind,
                    entity_id: event.entity_id,
                    body: event.body,
                    created_at: event.created_at,
                })
                .collect(),
            oldest_cursor: page.bounds.oldest,
            latest_cursor: page.bounds.latest,
            cursor_valid: page.cursor_valid,
        })
    }

    /// Returns bounded live projections used after a refresh or cursor reset.
    ///
    /// # Errors
    ///
    /// Returns a stable persistence error when checkpoints cannot be read.
    pub async fn progress_checkpoints(&self) -> Result<Vec<Value>, ApiError> {
        self.store
            .load_progress()
            .await
            .map_err(store_error)
            .map(|values| values.into_iter().map(|value| value.body).collect())
    }

    /// Scans replay-safe startup work without changing Run ownership. The
    /// returned ids are claimed only after the recovery supervisor owns each
    /// Project workspace lease.
    ///
    /// # Errors
    ///
    /// Returns a persistence error when the Run index cannot be read.
    pub async fn prepare_startup_recovery(&self) -> Result<StartupRecoveryPlan, ApiError> {
        let state = self
            .read_records(vec![ControlFilter::all(ControlRecordKind::Run)])
            .await?
            .original;
        Ok(StartupRecoveryPlan {
            run_ids: state
                .runs
                .iter()
                .filter(|run| !is_terminal_workspace_status(&run.status))
                .map(|run| run.id.clone())
                .collect(),
        })
    }

    /// Executes a previously claimed startup plan after the daemon listener is
    /// ready. Local Project/Git failures interrupt only their owning Run.
    ///
    /// # Errors
    ///
    /// Returns when global durable state can no longer be read or committed.
    pub async fn run_startup_recovery(
        &self,
        plan: StartupRecoveryPlan,
    ) -> Result<Vec<RunView>, ApiError> {
        let mut recovered = Vec::new();
        for run_id in plan.run_ids {
            let workspace_lease = match self.acquire_workspace_write_for_run(&run_id).await {
                Ok(lease) => lease,
                Err(failure) if failure.code == ErrorCode::ProjectWorkspaceBusy => {
                    // A live executor still owns this Project. It also owns the
                    // only path to Git publication, so do not steal its epoch.
                    continue;
                }
                Err(failure) if is_local_recovery_failure(failure.code) => {
                    recovered.push(self.interrupt_recovery_run(&run_id, &failure).await?);
                    continue;
                }
                Err(failure) => return Err(failure),
            };
            let claim = self.claim_startup_recovery(&run_id).await?;
            let lease = match claim {
                WorkspaceRecoveryClaim::Recovered(run) => {
                    recovered.push(*run);
                    continue;
                }
                WorkspaceRecoveryClaim::Skip => continue,
                WorkspaceRecoveryClaim::Execute => None,
                WorkspaceRecoveryClaim::Finalize(lease) => Some(lease),
            };
            let control = Arc::new(WorkspaceRunControl::new());
            if let Some(lease) = lease.as_ref() {
                control.bind_integration_lease(self.clone(), lease.clone())?;
            }
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
            let result = match lease {
                None => {
                    self.supervise_workspace_agent(run_id.clone(), control.clone())
                        .await
                }
                Some(lease) => self.recover_checkpointed_run(&lease, control.clone()).await,
            };
            drop((workspace_lease, invocation, control_guard, control));
            match result {
                Ok(run) => recovered.push(run),
                Err(failure) => return Err(failure),
            }
        }
        Ok(recovered)
    }

    #[allow(
        clippy::too_many_lines,
        reason = "the ownership claim keeps policy, lease fencing, and its event in one CAS"
    )]
    async fn claim_startup_recovery(
        &self,
        run_id: &str,
    ) -> Result<WorkspaceRecoveryClaim, ApiError> {
        for _ in 0..4 {
            let loaded = self.read_run_records(run_id).await?;
            let mut state = loaded.original.clone();
            let index = state
                .runs
                .iter()
                .position(|run| run.id == run_id)
                .ok_or_else(|| error(ErrorCode::InvalidRun, "run not found", false))?;
            if is_terminal_workspace_status(&state.runs[index].status) {
                return Ok(WorkspaceRecoveryClaim::Skip);
            }
            let policy = recovery_policy(&state);
            let status = state.runs[index].status.clone();
            let claim = match policy {
                RecoveryPolicy::ResumeSafe if status == "queued" => {
                    return Ok(WorkspaceRecoveryClaim::Execute);
                }
                RecoveryPolicy::ResumeSafe if status == "settling" => {
                    let operation_id = state.runs[index].operation_id.as_deref().map(str::to_owned);
                    let valid = operation_id.as_ref().is_some_and(|operation_id| {
                        state
                            .workspace_run_journals
                            .get(run_id)
                            .is_some_and(|journal| {
                                journal.operation_id == *operation_id
                                    && journal.lease_epoch == state.runs[index].lease_epoch
                                    && journal.result.is_some()
                            })
                    });
                    if valid {
                        let operation_id = operation_id
                            .ok_or_else(|| recovery_error("validated operation id disappeared"))?;
                        let lease_epoch = state.runs[index].lease_epoch.saturating_add(1);
                        state
                            .workspace_run_journals
                            .get_mut(run_id)
                            .ok_or_else(|| recovery_error("validated result journal disappeared"))?
                            .lease_epoch = lease_epoch;
                        let run = &mut state.runs[index];
                        run.lease_epoch = lease_epoch;
                        run.phase = Some("reconciling_result".into());
                        run.error = None;
                        WorkspaceRecoveryClaim::Finalize(WorkspaceExecutionLease {
                            run_id: run_id.to_owned(),
                            operation_id,
                            lease_epoch,
                        })
                    } else {
                        settle_recovered_run(
                            &mut state,
                            index,
                            "interrupted",
                            "checkpointed workspace result is incomplete; recovery material was preserved for review",
                        );
                        WorkspaceRecoveryClaim::Recovered(Box::new(state.runs[index].clone()))
                    }
                }
                RecoveryPolicy::ResumeSafe if status == "cancelling" => {
                    settle_recovered_run(
                        &mut state,
                        index,
                        "cancelled",
                        "run cancellation was completed during daemon recovery",
                    );
                    WorkspaceRecoveryClaim::Recovered(Box::new(state.runs[index].clone()))
                }
                RecoveryPolicy::ResumeSafe => {
                    settle_recovered_run(
                        &mut state,
                        index,
                        "interrupted",
                        "run effects are not proven replay-safe; isolated workspace recovery material was preserved for review",
                    );
                    WorkspaceRecoveryClaim::Recovered(Box::new(state.runs[index].clone()))
                }
                RecoveryPolicy::Ask => {
                    settle_recovered_run(
                        &mut state,
                        index,
                        "interrupted",
                        "recovery policy requires user review; workspace changes were preserved",
                    );
                    WorkspaceRecoveryClaim::Recovered(Box::new(state.runs[index].clone()))
                }
                RecoveryPolicy::Fail => {
                    settle_recovered_run(
                        &mut state,
                        index,
                        "failed",
                        "run failed because the daemon restarted",
                    );
                    WorkspaceRecoveryClaim::Recovered(Box::new(state.runs[index].clone()))
                }
            };
            let run = &state.runs[index];
            let event = pending(
                if is_terminal_workspace_status(&run.status) {
                    "run.recovered"
                } else {
                    "run.recovery_claimed"
                },
                Some(run.id.clone()),
                run,
            );
            match self.persist_records(&loaded, &state, vec![event]).await {
                Ok(()) => return Ok(claim),
                Err(ControlStoreError::Conflict) => {}
                Err(error) => return Err(store_error(error)),
            }
        }
        Err(error(
            ErrorCode::RunQueueConflict,
            "startup Run ownership claim did not settle",
            true,
        ))
    }

    /// Synchronous convenience for tests and non-daemon embeddings.
    ///
    /// # Errors
    ///
    /// Returns a global persistence error from either startup phase.
    pub async fn recover_interrupted_runs(&self) -> Result<Vec<RunView>, ApiError> {
        let plan = self.prepare_startup_recovery().await?;
        self.run_startup_recovery(plan).await
    }

    async fn recover_checkpointed_run(
        &self,
        lease: &WorkspaceExecutionLease,
        control: Arc<WorkspaceRunControl>,
    ) -> Result<RunView, ApiError> {
        let state = self.read_run_records(&lease.run_id).await?.original;
        let run = state
            .runs
            .iter()
            .find(|run| run.id == lease.run_id)
            .ok_or_else(|| error(ErrorCode::InvalidRun, "run not found", false))?;
        ensure_current_lease(run, lease)?;
        let journal = state
            .workspace_run_journals
            .get(&lease.run_id)
            .ok_or_else(|| recovery_error("workspace result journal is missing"))?;
        ensure_journal_lease(journal, lease)?;
        let result = journal
            .result
            .clone()
            .ok_or_else(|| recovery_error("workspace result checkpoint is missing"))?;
        let executor = self.workspace_agent.as_ref().ok_or_else(|| {
            error(
                ErrorCode::InvalidConfiguration,
                "Codex workspace executor is not configured",
                false,
            )
        })?;
        control
            .begin_integration()
            .await
            .map_err(|failure| error(failure.code, failure.message, failure.retryable))?;
        let invocation = workspace_invocation(&state, run, control, Arc::new(self.clone()))
            .map_err(|failure| error(failure.code, failure.message, failure.retryable))?;
        let output = match executor
            .recover_checkpointed(invocation, result, journal.baseline_ref.clone())
            .await
        {
            Ok(output) => output,
            Err(failure) => {
                let failure = error(failure.code, failure.message, failure.retryable);
                return self.interrupt_recovery_run(&lease.run_id, &failure).await;
            }
        };
        self.finish_workspace_run(lease, Ok(output)).await
    }

    async fn interrupt_recovery_run(
        &self,
        run_id: &str,
        failure: &ApiError,
    ) -> Result<RunView, ApiError> {
        for _ in 0..4 {
            let loaded = self.read_run_records(run_id).await?;
            let mut state = loaded.original.clone();
            let index = state
                .runs
                .iter()
                .position(|run| run.id == run_id)
                .ok_or_else(|| error(ErrorCode::InvalidRun, "run not found", false))?;
            if is_terminal_workspace_status(&state.runs[index].status) {
                return Ok(state.runs[index].clone());
            }
            let run = &mut state.runs[index];
            run.lease_epoch = run.lease_epoch.saturating_add(1);
            run.status = "interrupted".into();
            run.phase = Some("terminal".into());
            run.error = Some(error(ErrorCode::RunRecoveryFailed, &failure.message, false));
            expire_pending_native_approvals(run, NativeApprovalStatus::Expired);
            if let Some(journal) = state.workspace_run_journals.get_mut(run_id) {
                journal.lease_epoch = run.lease_epoch;
            }
            let run = state.runs[index].clone();
            release_session(&mut state, &run);
            let event = pending("run.recovery_required", Some(run_id.to_owned()), &run);
            match self.persist_records(&loaded, &state, vec![event]).await {
                Ok(()) => {
                    self.notify_approval_waiters(&run);
                    let _ = self.store.clear_progress(run_id).await;
                    return Ok(run);
                }
                Err(ControlStoreError::Conflict) => {}
                Err(error) => return Err(store_error(error)),
            }
        }
        Err(error(
            ErrorCode::RunQueueConflict,
            "interrupted Run recovery did not settle",
            true,
        ))
    }

    async fn try_execute(&self, command: Command) -> Result<CommandResult, ApiError> {
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
            return read_command(loaded.original, loaded.revision, command);
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
        // Workspace-writing Codex requests for the same canonical Git root are
        // serialized before the user Message captures HEAD. Unrelated Projects
        // and read-only operations do not share this lease.
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

    async fn commit_with_finalization_gate(
        &self,
        command: Command,
        has_workspace_lease: bool,
        derive_source_locked: bool,
    ) -> Result<CommandOutcome, ApiError> {
        let cancellation_run_id = match &command {
            Command::CancelRun { run_id }
            | Command::ResolveNativeApproval {
                run_id,
                action: NativeApprovalAction::Cancel,
                ..
            } => Some(run_id),
            _ => None,
        };
        let control = cancellation_run_id.and_then(|run_id| {
            self.workspace_run_controls
                .lock()
                .expect("workspace run controls")
                .get(run_id)
                .and_then(Weak::upgrade)
        });
        let Some(control) = control else {
            let outcome = self
                .commit_command(command, has_workspace_lease, derive_source_locked)
                .await?;
            if let CommandOutcome::Ready(result) = &outcome
                && let CommandResult::Run(run) = result.as_ref()
                && run.status == "cancelled"
                && let Some(token) = self
                    .cancellations
                    .lock()
                    .expect("cancellations")
                    .get(&run.id)
            {
                token.cancel();
            }
            return Ok(outcome);
        };

        let mut decision = control.finalization.lock().await;
        if *decision == WorkspaceFinalizationDecision::Integrating {
            return Err(error(
                ErrorCode::RunAlreadyTerminal,
                "workspace integration has started; cancellation cannot replace its durable result",
                false,
            ));
        }
        let outcome = self
            .commit_command(command, has_workspace_lease, derive_source_locked)
            .await?;
        if let CommandOutcome::Ready(result) = &outcome
            && let CommandResult::Run(run) = result.as_ref()
            && run.status == "cancelled"
        {
            *decision = WorkspaceFinalizationDecision::Cancelled;
            control.cancellation.cancel();
        }
        Ok(outcome)
    }

    async fn commit_command(
        &self,
        command: Command,
        has_workspace_lease: bool,
        derive_source_locked: bool,
    ) -> Result<CommandOutcome, ApiError> {
        for _ in 0..4 {
            let loaded = self.read_command_records(&command).await?;
            let mut state = loaded.original.clone();
            check_session_admission(&state, &command)?;
            if !has_workspace_lease
                && workspace_write_path(&state, &command, self.permission_limits)?.is_some()
            {
                return Err(error(
                    ErrorCode::ProjectWorkspaceBusy,
                    "Agent configuration changed to a workspace-writing provider during admission; retry the request",
                    true,
                ));
            }
            let git_baseline = command_git_baseline(&state, &command)?;
            let (result, events) = apply_command(
                &mut state,
                command.clone(),
                git_baseline.as_ref(),
                self.permission_limits,
                derive_source_locked,
            )?;
            match self.persist_records(&loaded, &state, events).await {
                Ok(()) => {
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

    fn notify_approval_waiters(&self, run: &RunView) {
        let Ok(mut waiters) = self.approval_waiters.lock() else {
            return;
        };
        for approval in &run.native_approvals {
            if approval.status == NativeApprovalStatus::Pending {
                continue;
            }
            let Some(sender) = waiters.remove(&approval.id) else {
                continue;
            };
            let decision =
                approval_decision(approval).unwrap_or(WorkspaceApprovalDecision::Cancelled);
            sender.send_replace(Some(decision));
        }
    }

    async fn acquire_workspace_write(
        &self,
        command: &Command,
    ) -> Result<Option<WorkspaceWriteLease>, ApiError> {
        let state = self.read_command_records(command).await?.original;
        check_session_admission(&state, command)?;
        let Some(workdir) = workspace_write_path(&state, command, self.permission_limits)? else {
            return Ok(None);
        };
        self.acquire_workspace_path(&workdir).await.map(Some)
    }

    async fn acquire_workspace_write_for_run(
        &self,
        run_id: &str,
    ) -> Result<WorkspaceWriteLease, ApiError> {
        let state = self.read_run_records(run_id).await?.original;
        let run = state
            .runs
            .iter()
            .find(|run| run.id == run_id)
            .ok_or_else(|| error(ErrorCode::InvalidRun, "run not found", false))?;
        let project = state
            .projects
            .iter()
            .find(|project| project.id == run.project_id)
            .ok_or_else(|| error(ErrorCode::InvalidProject, "project not found", false))?;
        self.acquire_workspace_path(Path::new(&project.workdir))
            .await
    }

    async fn acquire_workspace_path(
        &self,
        workdir: &Path,
    ) -> Result<WorkspaceWriteLease, ApiError> {
        let canonical = workdir.canonicalize().map_err(|failure| {
            error(
                ErrorCode::ProjectPathNotFound,
                format!("cannot resolve Project workdir for write admission: {failure}"),
                false,
            )
        })?;
        let process_lock = {
            let mut leases = self.workspace_leases.lock().map_err(|_| {
                error(
                    ErrorCode::ProjectWorkspaceBusy,
                    "workspace write lease registry is unavailable",
                    true,
                )
            })?;
            leases.retain(|_, lease| lease.strong_count() > 0);
            if let Some(existing) = leases.get(&canonical).and_then(Weak::upgrade) {
                existing
            } else {
                let lease = Arc::new(tokio::sync::Mutex::new(()));
                leases.insert(canonical.clone(), Arc::downgrade(&lease));
                lease
            }
        };
        let process_guard = process_lock.lock_owned().await;
        let git_dir = absolute_git_dir(&canonical)?;
        let lock_dir = git_dir.join("ait").join("locks");
        std::fs::create_dir_all(&lock_dir).map_err(|failure| {
            error(
                ErrorCode::ProjectWorkspaceBusy,
                format!("cannot create workspace lease directory: {failure}"),
                true,
            )
        })?;
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(lock_dir.join("workspace-write.lock"))
            .map_err(|failure| {
                error(
                    ErrorCode::ProjectWorkspaceBusy,
                    format!("cannot open workspace write lease: {failure}"),
                    true,
                )
            })?;
        match file.try_lock() {
            Ok(()) => Ok(WorkspaceWriteLease {
                _process_guard: process_guard,
                _file: file,
            }),
            Err(std::fs::TryLockError::WouldBlock) => Err(error(
                ErrorCode::ProjectWorkspaceBusy,
                "another Ait process owns this Project workspace write lease",
                true,
            )),
            Err(std::fs::TryLockError::Error(failure)) => Err(error(
                ErrorCode::ProjectWorkspaceBusy,
                format!("cannot acquire Project workspace write lease: {failure}"),
                true,
            )),
        }
    }
}

#[async_trait::async_trait]
impl WorkspaceApproval for LocalControlService {
    async fn decide(
        &self,
        request: WorkspaceApprovalRequest,
    ) -> Result<WorkspaceApprovalDecision, DomainError> {
        let approval = native_approval_record(&request).map_err(api_domain_error)?;
        let approval_id = approval.id.clone();
        let cancellation = self
            .cancellations
            .lock()
            .ok()
            .and_then(|cancellations| cancellations.get(&request.run_id).cloned())
            .ok_or_else(|| {
                DomainError::invariant(
                    ErrorCode::RunNotResumable,
                    "native approval has no active Run supervisor",
                )
            })?;
        let (sender, mut receiver) = tokio::sync::watch::channel(None);
        {
            let mut waiters = self.approval_waiters.lock().map_err(|_| {
                DomainError::invariant(
                    ErrorCode::ToolApprovalRequired,
                    "native approval registry is unavailable",
                )
            })?;
            if waiters.contains_key(&approval_id) {
                return Err(DomainError::invariant(
                    ErrorCode::ToolCallDuplicate,
                    "duplicate native approval request",
                ));
            }
            waiters.insert(approval_id.clone(), sender);
        }
        let saved = match self.persist_native_approval(approval).await {
            Ok(saved) => saved,
            Err(failure) => {
                self.approval_waiters
                    .lock()
                    .ok()
                    .and_then(|mut waiters| waiters.remove(&approval_id));
                return Err(api_domain_error(failure));
            }
        };
        if saved.status != NativeApprovalStatus::Pending {
            self.approval_waiters
                .lock()
                .ok()
                .and_then(|mut waiters| waiters.remove(&approval_id));
            return approval_decision(&saved).ok_or_else(|| {
                DomainError::invariant(
                    ErrorCode::ToolApprovalRequired,
                    "native approval has no usable decision",
                )
            });
        }
        let decision = loop {
            tokio::select! {
                biased;
                () = cancellation.cancelled() => break WorkspaceApprovalDecision::Cancelled,
                changed = receiver.changed() => {
                    if changed.is_err() {
                        break WorkspaceApprovalDecision::Cancelled;
                    }
                    if let Some(decision) = receiver.borrow_and_update().clone() {
                        break decision;
                    }
                }
            }
        };
        self.approval_waiters
            .lock()
            .ok()
            .and_then(|mut waiters| waiters.remove(&approval_id));
        Ok(decision)
    }

    async fn expire(&self, request: &WorkspaceApprovalRequest) -> Result<(), DomainError> {
        let approval_id = native_approval_id(&request.run_id, &request.protocol_request_id)
            .map_err(api_domain_error)?;
        for _ in 0..4 {
            let loaded = self
                .read_run_records(&request.run_id)
                .await
                .map_err(api_domain_error)?;
            let mut state = loaded.original.clone();
            let Some(run) = state.runs.iter_mut().find(|run| run.id == request.run_id) else {
                return Err(DomainError::invariant(
                    ErrorCode::InvalidRun,
                    "native approval Run not found",
                ));
            };
            let Some(approval) = run
                .native_approvals
                .iter_mut()
                .find(|approval| approval.id == approval_id)
            else {
                return Ok(());
            };
            if approval.status != NativeApprovalStatus::Pending {
                return Ok(());
            }
            approval.status = NativeApprovalStatus::Expired;
            approval.decided_at = Some(now());
            if !run
                .native_approvals
                .iter()
                .any(|candidate| candidate.status == NativeApprovalStatus::Pending)
                && !is_terminal_workspace_status(&run.status)
            {
                run.phase = Some("calling_agent".into());
            }
            let run = run.clone();
            let event = pending("run.approval_expired", Some(request.run_id.clone()), &run);
            match self.persist_records(&loaded, &state, vec![event]).await {
                Ok(()) => {
                    self.notify_approval_waiters(&run);
                    return Ok(());
                }
                Err(ControlStoreError::Conflict) => {}
                Err(failure) => return Err(api_domain_error(store_error(failure))),
            }
        }
        Err(DomainError::transient(
            ErrorCode::RunQueueConflict,
            "native approval expiry did not settle",
        ))
    }
}

impl LocalControlService {
    async fn persist_native_approval(
        &self,
        approval: NativeApprovalView,
    ) -> Result<NativeApprovalView, ApiError> {
        for _ in 0..4 {
            let loaded = self.read_run_records(&approval.run_id).await?;
            let mut state = loaded.original.clone();
            let run = state
                .runs
                .iter_mut()
                .find(|run| run.id == approval.run_id)
                .ok_or_else(|| error(ErrorCode::InvalidRun, "run not found", false))?;
            if is_terminal_workspace_status(&run.status) {
                return Err(error(
                    ErrorCode::RunAlreadyTerminal,
                    "native approval belongs to a terminal Run",
                    false,
                ));
            }
            if let Some(existing) = run
                .native_approvals
                .iter()
                .find(|existing| existing.id == approval.id)
            {
                if existing.protocol_request_id != approval.protocol_request_id
                    || existing.thread_id != approval.thread_id
                    || existing.turn_id != approval.turn_id
                    || existing.item_id != approval.item_id
                    || existing.target != approval.target
                    || existing.requested_permissions != approval.requested_permissions
                {
                    return Err(error(
                        ErrorCode::ToolCallDuplicate,
                        "native approval identity was reused with different correlation",
                        false,
                    ));
                }
                return Ok(existing.clone());
            }
            run.native_approvals.push(approval.clone());
            run.phase = Some("waiting_approval".into());
            let run = run.clone();
            let event = pending("run.approval_requested", Some(run.id.clone()), &run);
            match self.persist_records(&loaded, &state, vec![event]).await {
                Ok(()) => return Ok(approval),
                Err(ControlStoreError::Conflict) => {}
                Err(failure) => return Err(store_error(failure)),
            }
        }
        Err(error(
            ErrorCode::RunQueueConflict,
            "native approval request did not settle",
            true,
        ))
    }
}

fn workspace_invocation(
    state: &WorkingSet,
    run: &RunView,
    control: Arc<WorkspaceRunControl>,
    approvals: Arc<dyn WorkspaceApproval>,
) -> Result<WorkspaceAgentInvocation, DomainError> {
    let project = state
        .projects
        .iter()
        .find(|project| project.id == run.project_id)
        .ok_or_else(|| DomainError::invariant(ErrorCode::InvalidProject, "project not found"))?;
    let user_text = state
        .messages
        .iter()
        .find(|message| message.id == run.base_message_id)
        .and_then(|message| message.text.clone())
        .ok_or_else(|| DomainError::invariant(ErrorCode::MessageNotFound, "run input not found"))?;
    let message_baseline = state
        .messages
        .iter()
        .find(|message| message.id == run.base_message_id)
        .and_then(|message| message.git_commit.clone());
    let baseline_commit = run
        .workspace_base_commit
        .as_ref()
        .or(message_baseline.as_ref())
        .cloned()
        .ok_or_else(|| {
            DomainError::invariant(
                ErrorCode::ProjectGitHeadUnavailable,
                "Codex Run has no authorized Git baseline",
            )
        })?;
    let baseline_index_tree = run
        .workspace_base_index_tree
        .as_deref()
        .map(str::to_owned)
        .ok_or_else(|| {
            DomainError::invariant(
                ErrorCode::ProjectGitHeadUnavailable,
                "Codex Run has no authorized Git index baseline",
            )
        })?;
    let (project_instructions, prompt) =
        codex_prompt(state, &run.base_message_id).map_err(api_domain_error)?;
    Ok(WorkspaceAgentInvocation {
        request_id: run.id.clone(),
        model: run.config.model.clone(),
        reasoning_effort: run.config.reasoning_effort.clone(),
        project_instructions,
        prompt,
        commit_subject: user_text,
        cwd: PathBuf::from(&project.workdir),
        baseline_commit,
        baseline_index_tree,
        permission_profile: run.permission_profile,
        approvals,
        cancellation: control.cancellation.clone(),
        integration_gate: Some(control),
    })
}

fn ensure_current_lease(run: &RunView, lease: &WorkspaceExecutionLease) -> Result<(), ApiError> {
    if run.operation_id.as_deref() != Some(lease.operation_id.as_str())
        || run.lease_epoch != lease.lease_epoch
    {
        return Err(recovery_error("stale workspace execution lease"));
    }
    Ok(())
}

fn ensure_journal_lease(
    journal: &WorkspaceRunJournal,
    lease: &WorkspaceExecutionLease,
) -> Result<(), ApiError> {
    if journal.operation_id != lease.operation_id || journal.lease_epoch != lease.lease_epoch {
        return Err(recovery_error("stale workspace result journal lease"));
    }
    Ok(())
}

fn recovery_error(message: &str) -> ApiError {
    error(ErrorCode::RunRecoveryFailed, message, false)
}

fn recovery_policy(state: &WorkingSet) -> RecoveryPolicy {
    match state
        .settings
        .0
        .get("runtime.recovery")
        .and_then(Value::as_str)
    {
        Some("ask") => RecoveryPolicy::Ask,
        Some("fail") => RecoveryPolicy::Fail,
        _ => RecoveryPolicy::ResumeSafe,
    }
}

fn effective_permission_profile(
    settings: &SettingsDocument,
    provider: &AgentProvider,
    limits: PermissionPolicyLimits,
) -> Result<RunPermissionProfile, ApiError> {
    if provider.kind != AgentMode::Codex {
        return Ok(RunPermissionProfile::default());
    }
    let sandbox = match settings
        .0
        .get("permissions.sandbox")
        .and_then(Value::as_str)
    {
        Some("read_only" | "strict") => SandboxAccess::ReadOnly,
        Some("workspace_write") => SandboxAccess::WorkspaceWrite,
        Some("full_access") => SandboxAccess::FullAccess,
        Some(value) => {
            return Err(error(
                ErrorCode::InvalidConfiguration,
                format!("unsupported Codex sandbox policy {value:?}"),
                false,
            ));
        }
        None => {
            return Err(error(
                ErrorCode::InvalidConfiguration,
                "Codex sandbox policy is missing",
                false,
            ));
        }
    };
    if sandbox > limits.max_sandbox {
        return Err(error(
            ErrorCode::InvalidConfiguration,
            format!(
                "Codex sandbox policy {sandbox:?} exceeds the administrator limit {:?}",
                limits.max_sandbox
            ),
            false,
        ));
    }
    let approval = match settings
        .0
        .get("permissions.approval")
        .and_then(Value::as_str)
    {
        Some("on_request") => ApprovalMode::OnRequest,
        Some("untrusted_only") => ApprovalMode::UntrustedOnly,
        Some("always") => {
            return Err(error(
                ErrorCode::InvalidConfiguration,
                "Codex app-server cannot guarantee approval for every native operation; permissions.approval=always is unsupported",
                false,
            ));
        }
        Some(value) => {
            return Err(error(
                ErrorCode::InvalidConfiguration,
                format!("unsupported Codex approval policy {value:?}"),
                false,
            ));
        }
        None => {
            return Err(error(
                ErrorCode::InvalidConfiguration,
                "Codex approval policy is missing",
                false,
            ));
        }
    };
    Ok(RunPermissionProfile { sandbox, approval })
}

fn native_approval_record(
    request: &WorkspaceApprovalRequest,
) -> Result<NativeApprovalView, ApiError> {
    if request.run_id.trim().is_empty()
        || request.thread_id.trim().is_empty()
        || request.turn_id.trim().is_empty()
        || request.item_id.trim().is_empty()
        || request.method.trim().is_empty()
        || request.run_id.len() > 512
        || request.thread_id.len() > 512
        || request.turn_id.len() > 512
        || request.item_id.len() > 512
        || request.method.len() > 128
        || request.run_id.contains('\0')
        || request.thread_id.contains('\0')
        || request.turn_id.contains('\0')
        || request.item_id.contains('\0')
        || request.method.contains('\0')
    {
        return Err(error(
            ErrorCode::ToolApprovalRequired,
            "native approval correlation is incomplete",
            false,
        ));
    }
    let protocol_request_id = match &request.protocol_request_id {
        Value::String(value) if !value.is_empty() && value.len() <= 512 => {
            ProtocolRequestId::String(value.clone())
        }
        Value::Number(value) if value.is_i64() => {
            ProtocolRequestId::Integer(value.as_i64().expect("checked integer"))
        }
        _ => {
            return Err(error(
                ErrorCode::ToolApprovalRequired,
                "native approval request id must be a bounded string or integer",
                false,
            ));
        }
    };
    validate_native_approval_target(request.kind, &request.target)?;
    let requested_permissions = if request.kind == NativeApprovalKind::Permissions {
        let value = request.requested_permissions.clone().ok_or_else(|| {
            error(
                ErrorCode::ToolApprovalRequired,
                "permission approval omitted its explicit permission profile",
                false,
            )
        })?;
        let profile: NativePermissionProfile =
            serde_json::from_value(value).map_err(|failure| {
                error(
                    ErrorCode::ToolApprovalRequired,
                    format!("invalid Codex permission profile: {failure}"),
                    false,
                )
            })?;
        validate_native_permission_profile(&profile)?;
        if serde_json::to_vec(&profile)
            .map_err(|_| {
                error(
                    ErrorCode::ToolApprovalRequired,
                    "Codex permission profile is not serializable",
                    false,
                )
            })?
            .len()
            > 128 * 1_024
        {
            return Err(error(
                ErrorCode::ToolApprovalRequired,
                "Codex permission profile exceeds the persistence and UI limit",
                false,
            ));
        }
        Some(profile)
    } else {
        None
    };
    Ok(NativeApprovalView {
        id: native_approval_id(&request.run_id, &request.protocol_request_id)?,
        run_id: request.run_id.clone(),
        protocol_request_id,
        method: request.method.clone(),
        kind: request.kind,
        thread_id: request.thread_id.clone(),
        turn_id: request.turn_id.clone(),
        item_id: request.item_id.clone(),
        target: request.target.clone(),
        requested_permissions,
        status: NativeApprovalStatus::Pending,
        granted_scope: None,
        granted_permissions: None,
        created_at: now(),
        decided_at: None,
    })
}

fn native_approval_id(run_id: &str, request_id: &Value) -> Result<String, ApiError> {
    let request_id = serde_json::to_vec(request_id).map_err(|_| {
        error(
            ErrorCode::ToolApprovalRequired,
            "native approval request id is not serializable",
            false,
        )
    })?;
    let mut digest = Sha256::new();
    digest.update(run_id.as_bytes());
    digest.update([0]);
    digest.update(request_id);
    Ok(format!("native-{:x}", digest.finalize()))
}

fn validate_native_permission_profile(profile: &NativePermissionProfile) -> Result<(), ApiError> {
    let mut explicit = false;
    if let Some(network) = &profile.network {
        if network.enabled.is_none() {
            return Err(error(
                ErrorCode::ToolApprovalRequired,
                "network permission must explicitly state enabled",
                false,
            ));
        }
        explicit = true;
    }
    if let Some(file_system) = &profile.file_system {
        if file_system.glob_scan_max_depth == Some(0) {
            return Err(error(
                ErrorCode::ToolApprovalRequired,
                "filesystem glob scan depth must be positive",
                false,
            ));
        }
        if file_system.entries.len() > 128
            || file_system.read.len() > 128
            || file_system.write.len() > 128
        {
            return Err(error(
                ErrorCode::ToolApprovalRequired,
                "filesystem permission profile is too large",
                false,
            ));
        }
        for value in file_system.read.iter().chain(&file_system.write) {
            validate_permission_path(value)?;
        }
        for entry in &file_system.entries {
            match &entry.path {
                ait_contracts::NativeFileSystemPath::Path { path } => {
                    validate_permission_path(path)?;
                }
                ait_contracts::NativeFileSystemPath::GlobPattern { pattern } => {
                    validate_permission_path(pattern)?;
                }
                ait_contracts::NativeFileSystemPath::Special { value } => match value {
                    ait_contracts::NativeFileSystemSpecialPath::ProjectRoots {
                        subpath: Some(subpath),
                    } => validate_permission_path(subpath)?,
                    ait_contracts::NativeFileSystemSpecialPath::Unknown { .. } => {
                        return Err(error(
                            ErrorCode::ToolApprovalRequired,
                            "unknown special filesystem permission path",
                            false,
                        ));
                    }
                    _ => {}
                },
            }
        }
        explicit |= !file_system.entries.is_empty()
            || !file_system.read.is_empty()
            || !file_system.write.is_empty();
    }
    if !explicit {
        return Err(error(
            ErrorCode::ToolApprovalRequired,
            "permission profile does not contain an explicit capability",
            false,
        ));
    }
    Ok(())
}

fn validate_permission_path(value: &str) -> Result<(), ApiError> {
    if !is_bounded_display_value(value) {
        return Err(error(
            ErrorCode::ToolApprovalRequired,
            "permission path is empty or outside size limits",
            false,
        ));
    }
    Ok(())
}

fn validate_native_approval_target(
    kind: NativeApprovalKind,
    target: &NativeApprovalTarget,
) -> Result<(), ApiError> {
    let valid = match (kind, target) {
        (
            NativeApprovalKind::CommandExecution | NativeApprovalKind::LegacyCommand,
            NativeApprovalTarget::Command { command, cwd },
        ) => is_bounded_display_value(command) && is_bounded_display_value(cwd),
        (NativeApprovalKind::CommandExecution, NativeApprovalTarget::Network { host, .. }) => {
            is_bounded_display_value(host)
        }
        (
            NativeApprovalKind::FileChange | NativeApprovalKind::LegacyPatch,
            NativeApprovalTarget::FileChange {
                grant_root,
                changes,
            },
        ) => {
            changes.len() <= 128
                && (!changes.is_empty() || grant_root.is_some())
                && grant_root.as_deref().is_none_or(is_bounded_display_value)
                && changes
                    .iter()
                    .all(|change| is_bounded_display_value(&change.path))
        }
        (NativeApprovalKind::Permissions, NativeApprovalTarget::Permissions { cwd }) => {
            is_bounded_display_value(cwd)
        }
        _ => false,
    };
    if valid {
        Ok(())
    } else {
        Err(error(
            ErrorCode::ToolApprovalRequired,
            "native approval has no bounded, reviewable authorization target",
            false,
        ))
    }
}

fn is_bounded_display_value(value: &str) -> bool {
    !value.trim().is_empty() && value.len() <= 4_096 && !value.chars().any(char::is_control)
}

fn approval_decision(approval: &NativeApprovalView) -> Option<WorkspaceApprovalDecision> {
    match approval.status {
        NativeApprovalStatus::Approved => Some(WorkspaceApprovalDecision::Approved {
            scope: approval.granted_scope?,
            permissions: approval
                .granted_permissions
                .as_ref()
                .and_then(|profile| serde_json::to_value(profile).ok()),
        }),
        NativeApprovalStatus::Denied => Some(WorkspaceApprovalDecision::Denied),
        NativeApprovalStatus::Cancelled | NativeApprovalStatus::Expired => {
            Some(WorkspaceApprovalDecision::Cancelled)
        }
        NativeApprovalStatus::Pending => None,
    }
}

fn is_terminal_workspace_status(status: &str) -> bool {
    matches!(
        status,
        "completed" | "failed" | "cancelled" | "limit_exceeded" | "interrupted"
    )
}

fn is_local_recovery_failure(code: ErrorCode) -> bool {
    matches!(
        code,
        ErrorCode::ProjectWorkspaceBusy
            | ErrorCode::ProjectPathNotFound
            | ErrorCode::ProjectPathNotDirectory
            | ErrorCode::ProjectGitInitFailed
            | ErrorCode::ProjectGitDirty
            | ErrorCode::ProjectGitHeadUnavailable
            | ErrorCode::InvalidProject
            | ErrorCode::InvalidConfiguration
            | ErrorCode::RunNotResumable
    )
}

fn settle_recovered_run(state: &mut WorkingSet, index: usize, status: &str, message: &str) {
    let mut run = state.runs[index].clone();
    run.lease_epoch = run.lease_epoch.saturating_add(1);
    if let Some(journal) = state.workspace_run_journals.get_mut(&run.id) {
        journal.lease_epoch = run.lease_epoch;
    }
    run.status = status.into();
    run.phase = Some("terminal".into());
    run.error = Some(error(
        if status == "cancelled" {
            ErrorCode::RunCancelled
        } else {
            ErrorCode::RunRecoveryFailed
        },
        message,
        false,
    ));
    expire_pending_native_approvals(&mut run, NativeApprovalStatus::Expired);
    release_session(state, &run);
    state.runs[index] = run;
}

fn workspace_write_path(
    state: &WorkingSet,
    command: &Command,
    permission_limits: PermissionPolicyLimits,
) -> Result<Option<PathBuf>, ApiError> {
    let targets = match command {
        Command::SendMessage { session_id, .. } => {
            let session = state
                .sessions
                .iter()
                .find(|session| session.id == *session_id)
                .ok_or_else(|| error(ErrorCode::SessionNotFound, "session not found", false))?;
            vec![(session.project_id.as_str(), session.agent_id.as_str())]
        }
        Command::ForkSession {
            project_id,
            agent_id,
            ..
        } => vec![(project_id.as_str(), agent_id.as_str())],
        Command::DeriveSession {
            project_id,
            source_session_id,
            agent_id,
            ..
        } => {
            let source_session = state
                .sessions
                .iter()
                .find(|session| session.id == *source_session_id)
                .ok_or_else(|| error(ErrorCode::SessionNotFound, "session not found", false))?;
            vec![
                (project_id.as_str(), agent_id.as_str()),
                (
                    source_session.project_id.as_str(),
                    source_session.agent_id.as_str(),
                ),
            ]
        }
        Command::TriggerCron {
            cron_id,
            scheduled_at,
        } => {
            if state.runs.iter().any(|run| {
                run.cron_id.as_deref() == Some(cron_id.as_str())
                    && run.scheduled_at == Some(*scheduled_at)
            }) {
                return Ok(None);
            }
            state
                .crons
                .iter()
                .find(|cron| cron.id == *cron_id && cron.enabled)
                .map_or_else(Vec::new, |cron| {
                    vec![(cron.project_id.as_str(), cron.agent_id.as_str())]
                })
        }
        _ => Vec::new(),
    };
    let Some((project_id, _)) = targets.first().copied() else {
        return Ok(None);
    };
    let writes_workspace = targets.iter().try_fold(false, |writes, (_, agent_id)| {
        let agent = require_agent(state, agent_id)?;
        let provider = validate_config(state, &agent.config)?;
        if provider.kind != AgentMode::Codex {
            return Ok::<_, ApiError>(writes);
        }
        let _ = effective_permission_profile(&state.settings, provider, permission_limits)?;
        Ok(true)
    })?;
    if !writes_workspace {
        return Ok(None);
    }
    let project = state
        .projects
        .iter()
        .find(|project| project.id == project_id)
        .ok_or_else(|| error(ErrorCode::InvalidProject, "project not found", false))?;
    Ok(Some(PathBuf::from(&project.workdir)))
}

fn absolute_git_dir(workdir: &Path) -> Result<PathBuf, ApiError> {
    let output = ProcessCommand::new("git")
        .arg("-C")
        .arg(workdir)
        .args(["rev-parse", "--absolute-git-dir"])
        .output()
        .map_err(|failure| {
            error(
                ErrorCode::ProjectGitHeadUnavailable,
                format!("cannot locate Project Git directory: {failure}"),
                false,
            )
        })?;
    if !output.status.success() {
        return Err(error(
            ErrorCode::ProjectGitHeadUnavailable,
            String::from_utf8_lossy(&output.stderr).trim(),
            false,
        ));
    }
    let path = PathBuf::from(String::from_utf8_lossy(&output.stdout).trim());
    if !path.is_absolute() {
        return Err(error(
            ErrorCode::ProjectGitHeadUnavailable,
            "Git returned a non-absolute metadata directory",
            false,
        ));
    }
    Ok(path)
}

fn git_symbolic_head(workdir: &Path) -> Result<Option<String>, ApiError> {
    let output = ProcessCommand::new("git")
        .arg("-C")
        .arg(workdir)
        .args(["symbolic-ref", "--quiet", "HEAD"])
        .output()
        .map_err(|failure| {
            error(
                ErrorCode::ProjectGitHeadUnavailable,
                format!("cannot inspect Project branch identity: {failure}"),
                false,
            )
        })?;
    if output.status.success() {
        return Ok(Some(
            String::from_utf8_lossy(&output.stdout).trim().to_owned(),
        ));
    }
    if output.status.code() == Some(1) {
        Ok(None)
    } else {
        Err(error(
            ErrorCode::ProjectGitHeadUnavailable,
            String::from_utf8_lossy(&output.stderr).trim(),
            false,
        ))
    }
}

fn api_domain_error(error: ApiError) -> DomainError {
    DomainError {
        code: error.code,
        message: error.message,
        retryable: error.retryable,
        details: None,
        cause_id: None,
    }
}

fn read_command(
    state: WorkingSet,
    revision: u64,
    command: Command,
) -> Result<CommandResult, ApiError> {
    match command {
        Command::GetRun { run_id } => state
            .runs
            .into_iter()
            .find(|run| run.id == run_id)
            .map(CommandResult::Run)
            .ok_or_else(|| error(ErrorCode::InvalidRun, "run not found", false)),
        Command::ExportProject { project_id } => {
            export_project(&state, revision, &project_id).map(CommandResult::ProjectExport)
        }
        Command::GetSettings => Ok(CommandResult::Settings(settings_view(&state))),
        Command::ListProjects => Ok(CommandResult::Projects(state.projects)),
        Command::ListAgents => Ok(CommandResult::Agents(state.agents)),
        Command::ListAgentProviders => Ok(CommandResult::AgentProviders(state.providers)),
        Command::ListSessions { .. } => Ok(CommandResult::Sessions(state.sessions)),
        Command::ListMessages { .. } => Ok(CommandResult::Messages(state.messages)),
        Command::ListRuns { .. } => Ok(CommandResult::Runs(state.runs)),
        Command::ListCrons => Ok(CommandResult::Crons(state.crons)),
        _ => unreachable!("mutating command routed to read path"),
    }
}

#[allow(
    clippy::too_many_lines,
    reason = "Keep exhaustive command dispatch together"
)]
fn apply_command(
    state: &mut WorkingSet,
    command: Command,
    user_git_baseline: Option<&GitBaseline>,
    permission_limits: PermissionPolicyLimits,
    derive_source_locked: bool,
) -> Result<(CommandOutcome, Vec<PendingEvent>), ApiError> {
    let (result, events) = match command {
        Command::RegisterProject {
            id,
            name,
            workdir,
            repo_url,
        } => register_project(state, id, name, &workdir, repo_url),
        Command::SetProjectDefaultAgent {
            project_id,
            agent_id,
        } => set_project_default_agent(state, &project_id, &agent_id),
        Command::RegisterAgent { id, name, config } => register_agent(state, id, name, config),
        Command::UpdateAgent { id, name, config } => update_agent(state, &id, name, config),
        Command::SetSessionConfig { session_id, config } => {
            set_session_config(state, &session_id, config)
        }
        Command::SaveAgentProvider { .. }
        | Command::DiscoverProviderModels { .. }
        | Command::RefreshProviderModels { .. } => {
            unreachable!("provider command uses credential boundary")
        }
        Command::CreateSession {
            id,
            project_id,
            agent_id,
            at_message_id,
        } => create_session(state, id, project_id, &agent_id, at_message_id),
        Command::SetSessionAgent {
            session_id,
            agent_id,
        } => set_session_agent(state, &session_id, &agent_id),
        Command::RenameSession { session_id, name } => rename_session(state, &session_id, &name),
        Command::SetSessionTitle { session_id, title } => {
            set_session_title(state, &session_id, &title)
        }
        Command::SendMessage { session_id, text } => {
            return send_message(
                state,
                session_id,
                text,
                require_user_git_baseline(user_git_baseline)?,
                permission_limits,
            );
        }
        Command::ForkSession {
            id,
            project_id,
            agent_id,
            at_message_id,
            text,
        } => {
            return fork_session(
                state,
                ForkSessionInput {
                    id,
                    project_id,
                    agent_id,
                    at_message_id,
                    text,
                },
                require_user_git_baseline(user_git_baseline)?,
                permission_limits,
            );
        }
        Command::DeriveSession {
            id,
            project_id,
            source_session_id,
            agent_id,
            at_message_id,
            text,
        } => {
            return derive_session(
                state,
                ForkSessionInput {
                    id,
                    project_id,
                    agent_id,
                    at_message_id,
                    text,
                },
                &source_session_id,
                derive_source_locked,
                require_user_git_baseline(user_git_baseline)?,
                permission_limits,
            );
        }
        Command::CancelRun { run_id } => cancel_run(state, &run_id),
        Command::ResolveNativeApproval {
            run_id,
            approval_id,
            action,
            scope,
        } => resolve_native_approval(
            state,
            &run_id,
            &approval_id,
            action,
            scope,
            permission_limits,
        ),
        Command::CreateCron {
            id,
            name,
            project_id,
            base_message_id,
            agent_id,
            schedule,
            timezone,
        } => create_cron(
            state,
            id,
            name,
            project_id,
            base_message_id,
            agent_id,
            schedule,
            timezone,
        ),
        Command::SetCronEnabled { cron_id, enabled } => set_cron_enabled(state, &cron_id, enabled),
        Command::TriggerCron {
            cron_id,
            scheduled_at,
        } => {
            return trigger_cron(
                state,
                &cron_id,
                scheduled_at,
                user_git_baseline,
                permission_limits,
            );
        }
        Command::ImportProject { archive, workdir } => import_project(state, archive, &workdir),
        Command::SaveSettings {
            expected_revision,
            values,
        } => save_settings(state, expected_revision, values),
        Command::ResetSettings => Ok(reset_settings(state)),
        Command::GetRun { .. }
        | Command::ExportProject { .. }
        | Command::GetSettings
        | Command::ListProjects
        | Command::ListAgents
        | Command::ListAgentProviders
        | Command::ListSessions { .. }
        | Command::ListMessages { .. }
        | Command::ListRuns { .. }
        | Command::ListCrons => unreachable!("read command routed to write path"),
    }?;
    Ok((CommandOutcome::Ready(Box::new(result)), events))
}

fn set_project_default_agent(
    state: &mut WorkingSet,
    project_id: &str,
    agent_id: &str,
) -> Result<(CommandResult, Vec<PendingEvent>), ApiError> {
    require_named_agent(state, agent_id)?;
    let project = state
        .projects
        .iter_mut()
        .find(|project| project.id == project_id)
        .ok_or_else(|| error(ErrorCode::InvalidProject, "project not found", false))?;
    project.default_agent_id = Some(agent_id.to_owned());
    project.revision = project.revision.saturating_add(1);
    let project = project.clone();
    Ok((
        CommandResult::Project(project.clone()),
        vec![pending(
            "project.default_agent_updated",
            Some(project_id.to_owned()),
            &project,
        )],
    ))
}

fn fork_session(
    state: &mut WorkingSet,
    input: ForkSessionInput,
    git_baseline: &GitBaseline,
    permission_limits: PermissionPolicyLimits,
) -> Result<(CommandOutcome, Vec<PendingEvent>), ApiError> {
    let (_, mut events) = create_session(
        state,
        input.id.clone(),
        input.project_id,
        &input.agent_id,
        Some(input.at_message_id),
    )?;
    let (result, mut run_events) =
        send_message(state, input.id, input.text, git_baseline, permission_limits)?;
    events.append(&mut run_events);
    Ok((result, events))
}

fn derive_session(
    state: &mut WorkingSet,
    input: ForkSessionInput,
    source_session_id: &str,
    source_locked: bool,
    git_baseline: &GitBaseline,
    permission_limits: PermissionPolicyLimits,
) -> Result<(CommandOutcome, Vec<PendingEvent>), ApiError> {
    if input.text.trim().is_empty() {
        return Err(error(
            ErrorCode::InvalidMessageRole,
            "message text is required",
            false,
        ));
    }
    if input.id.trim().is_empty() || state.sessions.iter().any(|session| session.id == input.id) {
        return Err(error(
            ErrorCode::InvalidSession,
            "session id is empty or already exists",
            false,
        ));
    }
    if !state
        .projects
        .iter()
        .any(|project| project.id == input.project_id)
    {
        return Err(error(ErrorCode::InvalidProject, "project not found", false));
    }
    require_agent(state, &input.agent_id)?;
    let source_message = state
        .messages
        .iter()
        .find(|message| message.id == input.at_message_id)
        .ok_or_else(|| {
            error(
                ErrorCode::MessageNotFound,
                "branch message not found",
                false,
            )
        })?;
    if source_message.project_id != input.project_id {
        return Err(error(
            ErrorCode::SessionMessageProjectMismatch,
            "branch message belongs to another project",
            false,
        ));
    }
    let source_session = state
        .sessions
        .iter()
        .find(|session| session.id == source_session_id)
        .ok_or_else(|| error(ErrorCode::SessionNotFound, "session not found", false))?
        .clone();
    if source_session.project_id != input.project_id {
        return Err(error(
            ErrorCode::SessionMessageProjectMismatch,
            "source Session belongs to another project",
            false,
        ));
    }

    let source_is_leaf = !state
        .messages
        .iter()
        .any(|message| message.parent_message_id.as_deref() == Some(input.at_message_id.as_str()));
    let can_reuse = source_locked
        && source_session.active_run_id.is_none()
        && source_session.current_message_id == input.at_message_id
        && source_session.agent_id == input.agent_id
        && source_is_leaf;
    if can_reuse {
        return send_message(
            state,
            source_session_id.to_owned(),
            input.text,
            git_baseline,
            permission_limits,
        );
    }
    fork_session(state, input, git_baseline, permission_limits)
}

fn require_user_git_baseline(baseline: Option<&GitBaseline>) -> Result<&GitBaseline, ApiError> {
    baseline.ok_or_else(|| {
        error(
            ErrorCode::ProjectGitHeadUnavailable,
            "Git HEAD/index snapshot is missing for user message",
            false,
        )
    })
}

fn settings_view(state: &WorkingSet) -> SettingsView {
    SettingsView {
        schema: settings_schema(),
        values: state.settings.clone(),
        revision: state.settings_revision,
    }
}

fn save_settings(
    state: &mut WorkingSet,
    expected_revision: u64,
    values: SettingsDocument,
) -> Result<(CommandResult, Vec<PendingEvent>), ApiError> {
    if state.settings_revision != expected_revision {
        return Err(error(
            ErrorCode::InvalidConfiguration,
            "settings changed in another client; reload and try again",
            false,
        ));
    }
    validate_settings(&values)?;
    state.settings = values;
    state.settings_revision = state.settings_revision.saturating_add(1);
    let view = settings_view(state);
    Ok((
        CommandResult::Settings(view.clone()),
        vec![pending("settings.updated", None, &view)],
    ))
}

fn reset_settings(state: &mut WorkingSet) -> (CommandResult, Vec<PendingEvent>) {
    state.settings = default_settings();
    state.settings_revision = state.settings_revision.saturating_add(1);
    let view = settings_view(state);
    (
        CommandResult::Settings(view.clone()),
        vec![pending("settings.reset", None, &view)],
    )
}

fn validate_settings(values: &SettingsDocument) -> Result<(), ApiError> {
    let schema = settings_schema();
    let expected = schema
        .definitions
        .iter()
        .map(|definition| definition.id.as_str())
        .collect::<HashSet<_>>();
    if values.0.keys().any(|key| !expected.contains(key.as_str())) {
        return Err(error(
            ErrorCode::InvalidConfiguration,
            "settings contain an unknown key",
            false,
        ));
    }
    for definition in schema.definitions {
        let Some(value) = values.0.get(&definition.id) else {
            return Err(error(
                ErrorCode::InvalidConfiguration,
                format!("missing setting {}", definition.id),
                false,
            ));
        };
        let valid = match &definition.kind {
            SettingKind::Text | SettingKind::Path | SettingKind::CredentialReference => {
                value.is_string()
            }
            SettingKind::Boolean => value.is_boolean(),
            SettingKind::Number { min, max } => value
                .as_i64()
                .is_some_and(|number| number >= *min && number <= *max),
            SettingKind::Select { options } => value
                .as_str()
                .is_some_and(|choice| options.iter().any(|option| option == choice)),
        };
        if !valid {
            return Err(error(
                ErrorCode::InvalidConfiguration,
                format!("invalid value for setting {}", definition.id),
                false,
            ));
        }
    }
    Ok(())
}

fn register_project(
    state: &mut WorkingSet,
    id: String,
    name: String,
    workdir: &str,
    mut repo_url: Option<String>,
) -> Result<(CommandResult, Vec<PendingEvent>), ApiError> {
    if id.trim().is_empty() || name.trim().is_empty() {
        return Err(error(
            ErrorCode::InvalidProject,
            "project id and name are required",
            false,
        ));
    }
    if state.projects.iter().any(|project| project.id == id) {
        return Err(error(
            ErrorCode::InvalidProject,
            "project id already exists",
            false,
        ));
    }
    if let Some(url) = &mut repo_url {
        *url = url.trim().to_owned();
        if url.is_empty() {
            return Err(error(
                ErrorCode::InvalidProject,
                "repository URL cannot be empty",
                false,
            ));
        }
    }
    let canonical = prepare_git_root(Path::new(&workdir))?;
    let base_commit = ensure_git_head(&canonical)?;
    let canonical_text = canonical.to_string_lossy().into_owned();
    if state
        .projects
        .iter()
        .any(|project| project.workdir == canonical_text)
    {
        return Err(error(
            ErrorCode::ProjectPathAlreadyRegistered,
            "project path is already registered",
            false,
        ));
    }
    let root_id = Uuid::new_v4().to_string();
    let project = ProjectView {
        id: id.clone(),
        name,
        workdir: canonical_text,
        root_message_id: root_id.clone(),
        repo_url,
        base_commit,
        default_agent_id: None,
        revision: 1,
    };
    state.messages.push(MessageView {
        id: root_id,
        project_id: id.clone(),
        parent_message_id: None,
        role: "system".into(),
        kind: "standard".into(),
        text: Some("AIT project instructions".into()),
        created_at: now(),
        git_commit: None,
        data: None,
    });
    state.projects.push(project.clone());
    Ok((
        CommandResult::Project(project.clone()),
        vec![pending("project.registered", Some(id), &project)],
    ))
}

fn create_session(
    state: &mut WorkingSet,
    id: String,
    project_id: String,
    agent_id: &str,
    at_message_id: Option<String>,
) -> Result<(CommandResult, Vec<PendingEvent>), ApiError> {
    if id.trim().is_empty() || state.sessions.iter().any(|session| session.id == id) {
        return Err(error(
            ErrorCode::InvalidSession,
            "session id is empty or already exists",
            false,
        ));
    }
    let project = state
        .projects
        .iter()
        .find(|project| project.id == project_id)
        .ok_or_else(|| error(ErrorCode::InvalidProject, "project not found", false))?;
    require_agent(state, agent_id)?;
    let head = at_message_id.unwrap_or_else(|| project.root_message_id.clone());
    let target = state
        .messages
        .iter()
        .find(|message| message.id == head)
        .ok_or_else(|| {
            error(
                ErrorCode::MessageNotFound,
                "branch message not found",
                false,
            )
        })?;
    if target.project_id != project_id {
        return Err(error(
            ErrorCode::SessionMessageProjectMismatch,
            "branch message belongs to another project",
            false,
        ));
    }
    let agent_id = agent_for_session(state, agent_id, &id)?;
    let session = SessionView {
        id: id.clone(),
        project_id,
        name: String::new(),
        title: None,
        description: String::new(),
        title_generation_started: false,
        agent_id,
        current_message_id: head,
        active_run_id: None,
        version: 1,
    };
    state.sessions.push(session.clone());
    Ok((
        CommandResult::Session(session.clone()),
        vec![pending("session.created", Some(id), &session)],
    ))
}

fn rename_session(
    state: &mut WorkingSet,
    session_id: &str,
    name: &str,
) -> Result<(CommandResult, Vec<PendingEvent>), ApiError> {
    let name = name.split_whitespace().collect::<Vec<_>>().join(" ");
    if name.chars().count() > 100 {
        return Err(error(
            ErrorCode::InvalidSession,
            "Session name must be at most 100 characters",
            false,
        ));
    }
    let session = state
        .sessions
        .iter_mut()
        .find(|session| session.id == session_id)
        .ok_or_else(|| error(ErrorCode::SessionNotFound, "session not found", false))?;
    session.name = name;
    let session = session.clone();
    Ok((
        CommandResult::Session(session.clone()),
        vec![pending(
            "session.renamed",
            Some(session_id.to_owned()),
            &session,
        )],
    ))
}

fn set_session_title(
    state: &mut WorkingSet,
    session_id: &str,
    title: &str,
) -> Result<(CommandResult, Vec<PendingEvent>), ApiError> {
    let title = title.split_whitespace().collect::<Vec<_>>().join(" ");
    if title.is_empty() || title.chars().count() > 60 {
        return Err(error(
            ErrorCode::InvalidSession,
            "Temporary Session title must contain 1 to 60 characters",
            false,
        ));
    }
    let session = state
        .sessions
        .iter_mut()
        .find(|session| session.id == session_id)
        .ok_or_else(|| error(ErrorCode::SessionNotFound, "session not found", false))?;
    if !session.title_generation_started {
        session.title = Some(title);
    }
    let session = session.clone();
    Ok((
        CommandResult::Session(session.clone()),
        vec![pending(
            "session.title_updated",
            Some(session_id.to_owned()),
            &session,
        )],
    ))
}

fn is_first_completed_interaction(state: &WorkingSet, session: &SessionView) -> bool {
    let head_is_assistant = state
        .messages
        .iter()
        .find(|message| message.id == session.current_message_id)
        .is_some_and(|message| message.role == "assistant");
    head_is_assistant
        && state
            .runs
            .iter()
            .filter(|run| run.session_id.as_deref() == Some(session.id.as_str()))
            .count()
            == 1
}

fn validate_session_metadata(title: &str, description: &str) -> Result<(), ApiError> {
    let title = title.trim();
    let invalid_markup = [
        '"', '\'', '“', '”', '‘', '’', '#', '*', '`', '[', ']', '<', '>',
    ];
    let ending_punctuation = [
        '.', '。', '!', '！', '?', '？', ',', '，', ';', '；', ':', '：',
    ];
    if title.is_empty()
        || title.chars().count() > 36
        || title
            .chars()
            .any(|character| invalid_markup.contains(&character))
        || title
            .chars()
            .last()
            .is_some_and(|character| ending_punctuation.contains(&character))
        || description.trim().is_empty()
    {
        return Err(error(
            ErrorCode::InvalidSession,
            "generated Session metadata is outside the requested constraints",
            false,
        ));
    }
    Ok(())
}

fn set_session_agent(
    state: &mut WorkingSet,
    session_id: &str,
    agent_id: &str,
) -> Result<(CommandResult, Vec<PendingEvent>), ApiError> {
    let agent_id = agent_for_session(state, agent_id, session_id)?;
    let session = state
        .sessions
        .iter_mut()
        .find(|session| session.id == session_id)
        .ok_or_else(|| error(ErrorCode::SessionNotFound, "session not found", false))?;
    if session.active_run_id.is_some() {
        return Err(error(
            ErrorCode::SessionBusy,
            "session already has an active run",
            false,
        ));
    }
    if session.agent_id == agent_id {
        return Ok((CommandResult::Session(session.clone()), Vec::new()));
    }
    session.agent_id = agent_id;
    session.version = session.version.saturating_add(1);
    let session = session.clone();
    Ok((
        CommandResult::Session(session.clone()),
        vec![pending(
            "session.agent_updated",
            Some(session_id.to_owned()),
            &session,
        )],
    ))
}

fn send_message(
    state: &mut WorkingSet,
    session_id: String,
    text: String,
    git_baseline: &GitBaseline,
    permission_limits: PermissionPolicyLimits,
) -> Result<(CommandOutcome, Vec<PendingEvent>), ApiError> {
    if text.trim().is_empty() {
        return Err(error(
            ErrorCode::InvalidMessageRole,
            "message text is required",
            false,
        ));
    }
    let index = state
        .sessions
        .iter()
        .position(|session| session.id == session_id)
        .ok_or_else(|| error(ErrorCode::SessionNotFound, "session not found", false))?;
    let session = state.sessions[index].clone();
    if session.active_run_id.is_some() {
        return Err(error(
            ErrorCode::SessionBusy,
            "session already has an active run",
            false,
        ));
    }
    let agent = require_agent(state, &session.agent_id)?.clone();
    let provider = validate_config(state, &agent.config)?.clone();
    let permission_profile =
        effective_permission_profile(&state.settings, &provider, permission_limits)?;
    let user = message(
        &session.project_id,
        Some(&session.current_message_id),
        "user",
        "standard",
        Some(text),
        Some(&git_baseline.commit),
        None,
    );
    state.messages.push(user.clone());
    let run_id = Uuid::new_v4().to_string();
    state.sessions[index]
        .current_message_id
        .clone_from(&user.id);
    state.sessions[index].version += 1;
    state.sessions[index].active_run_id = Some(run_id.clone());
    let workspace_base_commit =
        (provider.kind == AgentMode::Codex).then(|| git_baseline.commit.clone());
    let workspace_base_index_tree = (provider.kind == AgentMode::Codex)
        .then(|| git_baseline.index_tree.clone().into_boxed_str());
    let run = RunView {
        id: run_id.clone(),
        project_id: session.project_id,
        base_message_id: user.id,
        last_message_id: None,
        session_id: Some(session_id),
        agent_id: agent.id.clone(),
        agent_revision: agent.revision,
        config: agent.config.clone(),
        provider,
        permission_profile,
        native_approvals: Vec::new(),
        trigger: "manual".into(),
        cron_id: None,
        scheduled_at: None,
        workspace_base_commit,
        workspace_base_index_tree,
        status: "queued".into(),
        phase: Some("queued".into()),
        operation_id: Some(format!("workspace-{run_id}").into_boxed_str()),
        lease_epoch: 0,
        error: None,
    };
    if let Some(reference) = state.provider_credentials.get(&agent.config.provider_id) {
        state
            .run_credentials
            .insert(run_id.clone(), reference.clone());
    }
    state.runs.push(run);
    let run = state.runs.last().expect("new run exists").clone();
    let event = pending("run.updated", Some(run_id), &run);
    Ok((CommandOutcome::for_new_run(run), vec![event]))
}

fn codex_prompt(state: &WorkingSet, head_id: &str) -> Result<(Option<String>, String), ApiError> {
    let mut path = Vec::new();
    let mut current = Some(head_id);
    while let Some(id) = current {
        let message = state
            .messages
            .iter()
            .find(|message| message.id == id)
            .ok_or_else(|| {
                error(
                    ErrorCode::MessageNotFound,
                    "message path is incomplete",
                    false,
                )
            })?;
        path.push(message);
        current = message.parent_message_id.as_deref();
    }
    path.reverse();
    let mut instructions = Vec::new();
    let mut prompt = String::from("Conversation:\n");
    for message in path {
        if let Some(text) = &message.text {
            if message.role == "system" {
                instructions.push(text.as_str());
            } else {
                let _ = writeln!(prompt, "{}: {text}", message.role);
            }
        }
    }
    Ok((
        (!instructions.is_empty()).then(|| instructions.join("\n\n")),
        prompt,
    ))
}

fn append_output(state: &mut WorkingSet, run: &mut RunView, output: MessageView) {
    run.last_message_id = Some(output.id.clone());
    if let Some(session_id) = &run.session_id
        && let Some(session) = state
            .sessions
            .iter_mut()
            .find(|session| &session.id == session_id)
    {
        session.current_message_id.clone_from(&output.id);
        session.version += 1;
    }
    state.messages.push(output);
}

fn release_session(state: &mut WorkingSet, run: &RunView) {
    if let Some(session_id) = &run.session_id
        && let Some(session) = state.sessions.iter_mut().find(|session| {
            &session.id == session_id && session.active_run_id.as_deref() == Some(run.id.as_str())
        })
    {
        session.active_run_id = None;
        session.version += 1;
    }
}

fn cancel_run(
    state: &mut WorkingSet,
    run_id: &str,
) -> Result<(CommandResult, Vec<PendingEvent>), ApiError> {
    let index = state
        .runs
        .iter()
        .position(|run| run.id == run_id)
        .ok_or_else(|| error(ErrorCode::InvalidRun, "run not found", false))?;
    if is_terminal_workspace_status(&state.runs[index].status) {
        return Err(error(
            ErrorCode::RunAlreadyTerminal,
            "run is already terminal",
            false,
        ));
    }
    if state.runs[index].phase.as_deref() == Some("integrating") {
        return Err(error(
            ErrorCode::RunAlreadyTerminal,
            "workspace integration has started; cancellation cannot replace its durable result",
            false,
        ));
    }
    let mut run = state.runs[index].clone();
    run.lease_epoch = run.lease_epoch.saturating_add(1);
    if let Some(journal) = state.workspace_run_journals.get_mut(&run.id) {
        journal.lease_epoch = run.lease_epoch;
    }
    run.status = "cancelled".into();
    run.phase = Some("terminal".into());
    run.error = Some(error(ErrorCode::RunCancelled, "run was cancelled", false));
    expire_pending_native_approvals(&mut run, NativeApprovalStatus::Cancelled);
    release_session(state, &run);
    state.runs[index] = run.clone();
    Ok((
        CommandResult::Run(run.clone()),
        // Terminal Run transitions share one event contract so every client
        // schedules an authoritative view refresh. Renderers still accept
        // legacy run.cancelled events retained in older outboxes.
        vec![pending("run.updated", Some(run.id.clone()), &run)],
    ))
}

fn resolve_native_approval(
    state: &mut WorkingSet,
    run_id: &str,
    approval_id: &str,
    action: NativeApprovalAction,
    scope: Option<ApprovalGrantScope>,
    limits: PermissionPolicyLimits,
) -> Result<(CommandResult, Vec<PendingEvent>), ApiError> {
    let run_index = state
        .runs
        .iter()
        .position(|run| run.id == run_id)
        .ok_or_else(|| error(ErrorCode::InvalidRun, "run not found", false))?;
    let run = &state.runs[run_index];
    if is_terminal_workspace_status(&run.status) {
        return Err(error(
            ErrorCode::RunAlreadyTerminal,
            "native approval belongs to a terminal Run",
            false,
        ));
    }
    let approval_index = run
        .native_approvals
        .iter()
        .position(|approval| approval.id == approval_id)
        .ok_or_else(|| {
            error(
                ErrorCode::ToolApprovalRequired,
                "native approval request not found",
                false,
            )
        })?;
    if run.native_approvals[approval_index].status != NativeApprovalStatus::Pending {
        return Err(error(
            ErrorCode::ToolApprovalRequired,
            "native approval request is no longer pending",
            false,
        ));
    }
    if action == NativeApprovalAction::Cancel {
        if scope.is_some() {
            return Err(error(
                ErrorCode::InvalidConfiguration,
                "cancellation cannot carry an authorization scope",
                false,
            ));
        }
        return cancel_run(state, run_id);
    }
    let project_root = state
        .projects
        .iter()
        .find(|project| project.id == run.project_id)
        .map(|project| PathBuf::from(&project.workdir))
        .ok_or_else(|| error(ErrorCode::InvalidProject, "project not found", false))?;
    let run_profile = run.permission_profile;
    let approval = &mut state.runs[run_index].native_approvals[approval_index];
    match action {
        NativeApprovalAction::Approve => {
            let scope = scope.ok_or_else(|| {
                error(
                    ErrorCode::InvalidConfiguration,
                    "approval scope is required when approving",
                    false,
                )
            })?;
            validate_native_approval_grant(approval, scope, limits, run_profile, &project_root)?;
            approval.status = NativeApprovalStatus::Approved;
            approval.granted_scope = Some(scope);
            approval
                .granted_permissions
                .clone_from(&approval.requested_permissions);
        }
        NativeApprovalAction::Deny => {
            if scope.is_some() {
                return Err(error(
                    ErrorCode::InvalidConfiguration,
                    "denial cannot carry an authorization scope",
                    false,
                ));
            }
            approval.status = NativeApprovalStatus::Denied;
        }
        NativeApprovalAction::Cancel => unreachable!("handled as Run cancellation above"),
    }
    approval.decided_at = Some(now());
    let run = &mut state.runs[run_index];
    if !run
        .native_approvals
        .iter()
        .any(|candidate| candidate.status == NativeApprovalStatus::Pending)
    {
        run.phase = Some("calling_agent".into());
    }
    let run = run.clone();
    Ok((
        CommandResult::Run(run.clone()),
        vec![pending("run.approval_resolved", Some(run.id.clone()), &run)],
    ))
}

fn validate_native_approval_grant(
    approval: &NativeApprovalView,
    scope: ApprovalGrantScope,
    limits: PermissionPolicyLimits,
    run_profile: RunPermissionProfile,
    project_root: &Path,
) -> Result<(), ApiError> {
    if run_profile.sandbox > limits.max_sandbox {
        return Err(error(
            ErrorCode::InvalidConfiguration,
            "Run permission snapshot exceeds the administrator sandbox ceiling",
            false,
        ));
    }
    if scope == ApprovalGrantScope::Session && !limits.allow_session_approvals {
        return Err(error(
            ErrorCode::InvalidConfiguration,
            "session-scoped approvals are disabled by administrator policy",
            false,
        ));
    }
    if approval.kind == NativeApprovalKind::Permissions {
        let permissions = approval.requested_permissions.as_ref().ok_or_else(|| {
            error(
                ErrorCode::InvalidConfiguration,
                "permission approval has no explicit permission profile",
                false,
            )
        })?;
        if scope == ApprovalGrantScope::OneShot {
            return Err(error(
                ErrorCode::InvalidConfiguration,
                "Codex permission approvals support turn or session scope, not one-shot scope",
                false,
            ));
        }
        validate_permission_grant_ceiling(permissions, run_profile, limits, project_root)?;
    } else if scope == ApprovalGrantScope::Turn {
        return Err(error(
            ErrorCode::InvalidConfiguration,
            "Codex command and file approvals support one-shot or session scope, not turn scope",
            false,
        ));
    }
    Ok(())
}

fn validate_permission_grant_ceiling(
    permissions: &NativePermissionProfile,
    run_profile: RunPermissionProfile,
    limits: PermissionPolicyLimits,
    project_root: &Path,
) -> Result<(), ApiError> {
    let Some(file_system) = &permissions.file_system else {
        return Ok(());
    };
    let requests_write = !file_system.write.is_empty()
        || file_system
            .entries
            .iter()
            .any(|entry| entry.access == ait_contracts::NativeFileSystemAccess::Write);
    if requests_write
        && (run_profile.sandbox < SandboxAccess::WorkspaceWrite
            || limits.max_sandbox < SandboxAccess::WorkspaceWrite)
    {
        return Err(error(
            ErrorCode::InvalidConfiguration,
            "filesystem write grant exceeds the Run snapshot or administrator sandbox ceiling",
            false,
        ));
    }
    for path in file_system.read.iter().chain(&file_system.write) {
        ensure_permission_path_in_project(path, project_root)?;
    }
    for entry in &file_system.entries {
        match &entry.path {
            ait_contracts::NativeFileSystemPath::Path { path } => {
                ensure_permission_path_in_project(path, project_root)?;
            }
            ait_contracts::NativeFileSystemPath::Special {
                value: ait_contracts::NativeFileSystemSpecialPath::ProjectRoots { subpath },
            } => {
                if let Some(subpath) = subpath {
                    ensure_permission_path_in_project(subpath, project_root)?;
                }
            }
            ait_contracts::NativeFileSystemPath::GlobPattern { .. }
            | ait_contracts::NativeFileSystemPath::Special { .. } => {
                return Err(error(
                    ErrorCode::InvalidConfiguration,
                    "filesystem grant cannot be proven to stay inside the Project boundary",
                    false,
                ));
            }
        }
    }
    Ok(())
}

fn ensure_permission_path_in_project(path: &str, project_root: &Path) -> Result<(), ApiError> {
    use std::path::Component;

    let root = lexical_absolute_path(project_root)?;
    let candidate = Path::new(path);
    let joined = if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        root.join(candidate)
    };
    let mut normalized = PathBuf::new();
    for component in joined.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalized.pop() {
                    return Err(project_boundary_error());
                }
            }
            Component::Normal(value) => normalized.push(value),
        }
    }
    if !normalized.starts_with(&root) {
        return Err(project_boundary_error());
    }
    let canonical_root = std::fs::canonicalize(&root).map_err(|_| project_boundary_error())?;
    let mut existing = normalized.as_path();
    while !existing.exists() {
        existing = existing.parent().ok_or_else(project_boundary_error)?;
    }
    let canonical_existing =
        std::fs::canonicalize(existing).map_err(|_| project_boundary_error())?;
    if !canonical_existing.starts_with(canonical_root) {
        return Err(project_boundary_error());
    }
    Ok(())
}

fn lexical_absolute_path(path: &Path) -> Result<PathBuf, ApiError> {
    if !path.is_absolute() {
        return Err(project_boundary_error());
    }
    let mut normalized = PathBuf::new();
    for component in path.components() {
        use std::path::Component;
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalized.pop() {
                    return Err(project_boundary_error());
                }
            }
            Component::Normal(value) => normalized.push(value),
        }
    }
    Ok(normalized)
}

fn project_boundary_error() -> ApiError {
    error(
        ErrorCode::InvalidConfiguration,
        "filesystem grant escapes the Project boundary",
        false,
    )
}

fn expire_pending_native_approvals(run: &mut RunView, status: NativeApprovalStatus) {
    let decided_at = now();
    for approval in &mut run.native_approvals {
        if approval.status == NativeApprovalStatus::Pending {
            approval.status = status;
            approval.decided_at = Some(decided_at);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn create_cron(
    state: &mut WorkingSet,
    id: String,
    name: String,
    project_id: String,
    base_message_id: String,
    agent_id: String,
    schedule: String,
    timezone: String,
) -> Result<(CommandResult, Vec<PendingEvent>), ApiError> {
    if state.crons.iter().any(|cron| cron.id == id) {
        return Err(error(
            ErrorCode::InvalidCron,
            "cron id already exists",
            false,
        ));
    }
    let base = state
        .messages
        .iter()
        .find(|message| message.id == base_message_id)
        .ok_or_else(|| {
            error(
                ErrorCode::CronBaseMessageUnavailable,
                "cron base message not found",
                false,
            )
        })?;
    if base.project_id != project_id {
        return Err(error(
            ErrorCode::CronBaseMessageUnavailable,
            "cron base message belongs to another project",
            false,
        ));
    }
    require_named_agent(state, &agent_id).map_err(|_| {
        error(
            ErrorCode::CronAgentUnavailable,
            "cron agent unavailable",
            false,
        )
    })?;
    let domain = Cron {
        id: CronId::new(&id),
        name: name.clone(),
        project_id: ProjectId::new(&project_id),
        base_message_id: MessageId::parse(&base_message_id)
            .map_err(|_| error(ErrorCode::InvalidCron, "invalid base message id", false))?,
        agent_id: AgentId::new(&agent_id),
        schedule: schedule.clone(),
        timezone: timezone.clone(),
        enabled: true,
        concurrency_policy: CronConcurrencyPolicy::Forbid,
        misfire_policy: CronMisfirePolicy::RunOnce,
        max_runtime: None,
        next_run_at: ait_scheduler::next_occurrence(&schedule, &timezone, TimestampMs(now()))
            .map_err(|failure| error(failure.code, failure.message, failure.retryable))?,
        last_run_at: None,
        version: 1,
        created_at: TimestampMs(now()),
        updated_at: TimestampMs(now()),
    };
    domain
        .validate()
        .map_err(|failure| error(failure.code, failure.message, failure.retryable))?;
    let cron = CronView {
        id: id.clone(),
        name,
        project_id,
        base_message_id,
        agent_id,
        schedule,
        timezone,
        enabled: true,
    };
    state.crons.push(cron.clone());
    Ok((
        CommandResult::Cron(cron.clone()),
        vec![pending("cron.created", Some(id), &cron)],
    ))
}

fn set_cron_enabled(
    state: &mut WorkingSet,
    cron_id: &str,
    enabled: bool,
) -> Result<(CommandResult, Vec<PendingEvent>), ApiError> {
    let cron = state
        .crons
        .iter_mut()
        .find(|cron| cron.id == cron_id)
        .ok_or_else(|| error(ErrorCode::InvalidCron, "cron not found", false))?;
    cron.enabled = enabled;
    let cron = cron.clone();
    Ok((
        CommandResult::Cron(cron.clone()),
        vec![pending(
            "cron.enabled_changed",
            Some(cron.id.clone()),
            &cron,
        )],
    ))
}

fn trigger_cron(
    state: &mut WorkingSet,
    cron_id: &str,
    scheduled_at: i64,
    workspace_baseline: Option<&GitBaseline>,
    permission_limits: PermissionPolicyLimits,
) -> Result<(CommandOutcome, Vec<PendingEvent>), ApiError> {
    if let Some(existing) = state.runs.iter().find(|run| {
        run.cron_id.as_deref() == Some(cron_id) && run.scheduled_at == Some(scheduled_at)
    }) {
        return Ok((
            CommandOutcome::Ready(Box::new(CommandResult::Run(existing.clone()))),
            Vec::new(),
        ));
    }
    let cron = state
        .crons
        .iter()
        .find(|cron| cron.id == cron_id && cron.enabled)
        .cloned()
        .ok_or_else(|| error(ErrorCode::InvalidCron, "enabled cron not found", false))?;
    let agent = require_agent(state, &cron.agent_id)?.clone();
    let provider = validate_config(state, &agent.config)?.clone();
    let permission_profile =
        effective_permission_profile(&state.settings, &provider, permission_limits)?;
    if provider.kind == AgentMode::Codex && workspace_baseline.is_none() {
        return Err(error(
            ErrorCode::ProjectGitHeadUnavailable,
            "Codex Cron Run requires a Git baseline captured under the workspace lease",
            true,
        ));
    }
    let run_id = Uuid::new_v4().to_string();
    if let Some(reference) = state.provider_credentials.get(&agent.config.provider_id) {
        state
            .run_credentials
            .insert(run_id.clone(), reference.clone());
    }
    state.runs.push(RunView {
        id: run_id.clone(),
        project_id: cron.project_id,
        base_message_id: cron.base_message_id,
        last_message_id: None,
        session_id: None,
        agent_id: agent.id.clone(),
        agent_revision: agent.revision,
        config: agent.config.clone(),
        provider,
        permission_profile,
        native_approvals: Vec::new(),
        trigger: "cron".into(),
        cron_id: Some(cron.id),
        scheduled_at: Some(scheduled_at),
        workspace_base_commit: workspace_baseline.map(|baseline| baseline.commit.clone()),
        workspace_base_index_tree: workspace_baseline
            .map(|baseline| baseline.index_tree.clone().into_boxed_str()),
        status: "queued".into(),
        phase: Some("queued".into()),
        operation_id: Some(format!("workspace-{run_id}").into_boxed_str()),
        lease_epoch: 0,
        error: None,
    });
    let run = state.runs.last().expect("new run exists").clone();
    let event = pending("cron.run_triggered", Some(run_id), &run);
    Ok((CommandOutcome::for_new_run(run), vec![event]))
}

fn export_project(
    state: &WorkingSet,
    source_revision: u64,
    project_id: &str,
) -> Result<ProjectExport, ApiError> {
    let project = state
        .projects
        .iter()
        .find(|project| project.id == project_id)
        .cloned()
        .ok_or_else(|| error(ErrorCode::InvalidProject, "project not found", false))?;
    let messages = state
        .messages
        .iter()
        .filter(|message| message.project_id == project_id)
        .cloned()
        .collect::<Vec<_>>();
    let sessions = state
        .sessions
        .iter()
        .filter(|session| session.project_id == project_id)
        .cloned()
        .map(|mut session| {
            // An active Run is process-local state and cannot safely be resumed
            // from a portable archive.
            session.active_run_id = None;
            session
        })
        .collect::<Vec<_>>();
    let mut referenced_agents = sessions
        .iter()
        .map(|session| session.agent_id.as_str())
        .collect::<HashSet<_>>();
    if let Some(default_agent_id) = project.default_agent_id.as_deref() {
        referenced_agents.insert(default_agent_id);
    }
    let agents: Vec<_> = state
        .agents
        .iter()
        .filter(|agent| referenced_agents.contains(agent.id.as_str()))
        .cloned()
        .collect();
    let providers = state
        .providers
        .iter()
        .filter(|p| agents.iter().any(|a| a.config.provider_id == p.provider.id))
        .map(|p| p.provider.clone())
        .collect();
    let archive = ProjectExport {
        format_version: PROJECT_EXPORT_VERSION,
        source_revision,
        providers,
        project,
        agents,
        sessions,
        messages,
    };
    validate_project_export(&archive)?;
    Ok(archive)
}

fn import_project(
    state: &mut WorkingSet,
    archive: ProjectExport,
    workdir: &str,
) -> Result<(CommandResult, Vec<PendingEvent>), ApiError> {
    validate_project_export(&archive)?;
    if state
        .projects
        .iter()
        .any(|project| project.id == archive.project.id)
    {
        return Err(error(
            ErrorCode::InvalidProject,
            "project id already exists",
            false,
        ));
    }
    if archive.messages.iter().any(|imported| {
        state
            .messages
            .iter()
            .any(|existing| existing.id == imported.id)
    }) || archive.sessions.iter().any(|imported| {
        state
            .sessions
            .iter()
            .any(|existing| existing.id == imported.id)
    }) {
        return Err(error(
            ErrorCode::InvalidProject,
            "archive identity conflicts with existing workspace state",
            false,
        ));
    }
    for imported in &archive.agents {
        if let Some(existing) = state
            .agents
            .iter()
            .find(|existing| existing.id == imported.id)
            && existing != imported
        {
            return Err(error(
                ErrorCode::InvalidAgentConfiguration,
                "archive agent conflicts with an existing revision",
                false,
            ));
        }
    }

    for provider in &archive.providers {
        if let Some(existing) = state
            .providers
            .iter()
            .find(|p| p.provider.id == provider.id)
            && existing.provider != *provider
        {
            return Err(invalid_archive(
                "archive provider conflicts with an existing connection",
            ));
        }
    }
    let canonical = prepare_git_root(Path::new(workdir))?;
    let canonical_text = canonical.to_string_lossy().into_owned();
    if state
        .projects
        .iter()
        .any(|project| project.workdir == canonical_text)
    {
        return Err(error(
            ErrorCode::ProjectPathAlreadyRegistered,
            "project path is already registered",
            false,
        ));
    }

    let mut project = archive.project;
    project.workdir = canonical_text;
    project.base_commit = ensure_git_head(&canonical)?;
    for agent in archive.agents {
        if !state.agents.iter().any(|existing| existing.id == agent.id) {
            state.agents.push(agent);
        }
    }
    for provider in archive.providers {
        if !state.providers.iter().any(|p| p.provider.id == provider.id) {
            state.providers.push(AgentProviderView {
                provider,
                has_secret: false,
            });
        }
    }
    state.messages.extend(archive.messages);
    state.sessions.extend(archive.sessions);
    state.projects.push(project.clone());
    Ok((
        CommandResult::Project(project.clone()),
        vec![pending(
            "project.imported",
            Some(project.id.clone()),
            &project,
        )],
    ))
}

fn validate_project_export(archive: &ProjectExport) -> Result<(), ApiError> {
    if archive.format_version != PROJECT_EXPORT_VERSION
        || archive.source_revision == 0
        || archive.project.id.trim().is_empty()
        || archive.project.revision == 0
        || !is_git_commit(&archive.project.base_commit)
        || archive
            .project
            .repo_url
            .as_ref()
            .is_some_and(|url| url.trim().is_empty())
        || archive.messages.is_empty()
    {
        return Err(invalid_archive(
            "archive header or project revision is invalid",
        ));
    }

    let mut message_by_id = HashMap::with_capacity(archive.messages.len());
    for message in &archive.messages {
        if message.project_id != archive.project.id
            || Uuid::parse_str(&message.id).is_err()
            || (message.role == "user"
                && message.kind == "standard"
                && message
                    .git_commit
                    .as_deref()
                    .is_none_or(|commit| !is_git_commit(commit)))
            || (message.git_commit.is_some()
                && (message.role != "user" || message.kind != "standard"))
            || message_by_id.insert(message.id.as_str(), message).is_some()
        {
            return Err(invalid_archive(
                "archive message identity or project ownership is invalid",
            ));
        }
    }
    let Some(root) = message_by_id.get(archive.project.root_message_id.as_str()) else {
        return Err(invalid_archive("archive root message is missing"));
    };
    if root.parent_message_id.is_some() || root.role != "system" {
        return Err(invalid_archive("archive root message is invalid"));
    }
    for message in &archive.messages {
        let mut cursor = message;
        let mut seen = HashSet::new();
        while cursor.id != archive.project.root_message_id {
            if !seen.insert(cursor.id.as_str()) {
                return Err(invalid_archive("archive message graph contains a cycle"));
            }
            let Some(parent_id) = cursor.parent_message_id.as_deref() else {
                return Err(invalid_archive(
                    "archive message graph contains an unexpected root",
                ));
            };
            cursor = message_by_id
                .get(parent_id)
                .copied()
                .ok_or_else(|| invalid_archive("archive message parent is missing"))?;
        }
    }

    validate_archive_catalog(archive)?;
    let mut agent_ids = HashSet::with_capacity(archive.agents.len());
    if archive.agents.iter().any(|agent| {
        agent.id.trim().is_empty() || agent.revision == 0 || !agent_ids.insert(agent.id.as_str())
    }) {
        return Err(invalid_archive("archive agent revision is invalid"));
    }
    if archive
        .project
        .default_agent_id
        .as_deref()
        .is_some_and(|agent_id| !agent_ids.contains(agent_id))
    {
        return Err(invalid_archive(
            "archive Project default Agent binding is invalid",
        ));
    }
    let mut session_ids = HashSet::with_capacity(archive.sessions.len());
    for session in &archive.sessions {
        if session.project_id != archive.project.id
            || session.version == 0
            || session.active_run_id.is_some()
            || !session_ids.insert(session.id.as_str())
            || !message_by_id.contains_key(session.current_message_id.as_str())
            || !agent_ids.contains(session.agent_id.as_str())
        {
            return Err(invalid_archive(
                "archive session pointer, agent binding, or revision is invalid",
            ));
        }
    }
    Ok(())
}

fn invalid_archive(message: impl Into<String>) -> ApiError {
    error(ErrorCode::InvalidProject, message, false)
}

fn require_agent<'a>(state: &'a WorkingSet, id: &str) -> Result<&'a AgentView, ApiError> {
    state
        .agents
        .iter()
        .find(|agent| agent.id == id && agent.enabled)
        .ok_or_else(|| error(ErrorCode::AgentNotFound, "enabled agent not found", false))
}

fn message(
    project: &str,
    parent: Option<&str>,
    role: &str,
    kind: &str,
    text: Option<String>,
    git_commit: Option<&str>,
    data: Option<Value>,
) -> MessageView {
    MessageView {
        id: Uuid::new_v4().to_string(),
        project_id: project.into(),
        parent_message_id: parent.map(str::to_owned),
        role: role.into(),
        kind: kind.into(),
        text,
        git_commit: git_commit.map(str::to_owned),
        data,
        created_at: now(),
    }
}

fn prepare_git_root(path: &Path) -> Result<std::path::PathBuf, ApiError> {
    if !path.exists() {
        return Err(error(
            ErrorCode::ProjectPathNotFound,
            "project path does not exist",
            false,
        ));
    }
    if !path.is_dir() {
        return Err(error(
            ErrorCode::ProjectPathNotDirectory,
            "project path is not a directory",
            false,
        ));
    }
    let canonical = path
        .canonicalize()
        .map_err(|failure| error(ErrorCode::ProjectPathNotFound, failure.to_string(), false))?;
    let top = git_top_level(&canonical);
    if top.as_deref() != Some(canonical.as_path()) {
        let output = ProcessCommand::new("git")
            .arg("-C")
            .arg(&canonical)
            .arg("init")
            .output()
            .map_err(|failure| {
                error(ErrorCode::ProjectGitInitFailed, failure.to_string(), false)
            })?;
        if !output.status.success() {
            return Err(error(
                ErrorCode::ProjectGitInitFailed,
                String::from_utf8_lossy(&output.stderr).into_owned(),
                false,
            ));
        }
    }
    if git_top_level(&canonical).as_deref() != Some(canonical.as_path()) {
        return Err(error(
            ErrorCode::ProjectGitInitFailed,
            "git root verification failed",
            false,
        ));
    }
    Ok(canonical)
}

fn ensure_git_head(path: &Path) -> Result<String, ApiError> {
    if let Some(head) = git_head(path)? {
        return Ok(head);
    }
    let staged = ProcessCommand::new("git")
        .arg("-C")
        .arg(path)
        .args(["diff", "--cached", "--quiet", "--exit-code"])
        .output()
        .map_err(|failure| {
            error(
                ErrorCode::ProjectGitHeadUnavailable,
                failure.to_string(),
                false,
            )
        })?;
    if !staged.status.success() {
        return Err(error(
            ErrorCode::ProjectGitHeadUnavailable,
            "cannot create an empty initial commit while the index contains staged changes",
            false,
        ));
    }
    let output = ProcessCommand::new("git")
        .arg("-C")
        .arg(path)
        .args([
            "-c",
            "user.name=AIT",
            "-c",
            "user.email=ait@localhost",
            "commit",
            "--allow-empty",
            "--no-gpg-sign",
            "--no-verify",
            "--quiet",
            "-m",
            "Initialize AIT project",
        ])
        .output()
        .map_err(|failure| {
            error(
                ErrorCode::ProjectGitHeadUnavailable,
                failure.to_string(),
                false,
            )
        })?;
    if !output.status.success() {
        return Err(error(
            ErrorCode::ProjectGitHeadUnavailable,
            String::from_utf8_lossy(&output.stderr).trim(),
            false,
        ));
    }
    git_head(path)?.ok_or_else(|| {
        error(
            ErrorCode::ProjectGitHeadUnavailable,
            "initial commit succeeded but Git HEAD is unavailable",
            false,
        )
    })
}

fn command_git_baseline(
    state: &WorkingSet,
    command: &Command,
) -> Result<Option<GitBaseline>, ApiError> {
    let project_id = match command {
        Command::SendMessage { session_id, .. } => Some(
            state
                .sessions
                .iter()
                .find(|session| session.id == *session_id)
                .ok_or_else(|| error(ErrorCode::SessionNotFound, "session not found", false))?
                .project_id
                .as_str(),
        ),
        Command::ForkSession { project_id, .. } | Command::DeriveSession { project_id, .. } => {
            Some(project_id.as_str())
        }
        Command::TriggerCron {
            cron_id,
            scheduled_at,
        } => {
            if state.runs.iter().any(|run| {
                run.cron_id.as_deref() == Some(cron_id.as_str())
                    && run.scheduled_at == Some(*scheduled_at)
            }) {
                return Ok(None);
            }
            let Some(cron) = state
                .crons
                .iter()
                .find(|cron| cron.id == *cron_id && cron.enabled)
            else {
                return Ok(None);
            };
            let agent = require_agent(state, &cron.agent_id)?;
            if validate_config(state, &agent.config)?.kind != AgentMode::Codex {
                return Ok(None);
            }
            Some(cron.project_id.as_str())
        }
        _ => return Ok(None),
    };
    let Some(project_id) = project_id else {
        return Ok(None);
    };
    let project = state
        .projects
        .iter()
        .find(|project| project.id == project_id)
        .ok_or_else(|| error(ErrorCode::InvalidProject, "project not found", false))?;
    clean_git_baseline(Path::new(&project.workdir)).map(Some)
}

fn clean_git_baseline(path: &Path) -> Result<GitBaseline, ApiError> {
    let before = git_head(path)?.ok_or_else(|| {
        error(
            ErrorCode::ProjectGitHeadUnavailable,
            "project repository has no HEAD commit",
            false,
        )
    })?;
    let index_before = git_index_tree(path)?;
    let status = ProcessCommand::new("git")
        .arg("-C")
        .arg(path)
        .args(["status", "--porcelain=v1", "--untracked-files=normal"])
        .output()
        .map_err(|failure| {
            error(
                ErrorCode::ProjectGitHeadUnavailable,
                failure.to_string(),
                false,
            )
        })?;
    if !status.status.success() {
        return Err(error(
            ErrorCode::ProjectGitHeadUnavailable,
            String::from_utf8_lossy(&status.stderr).trim(),
            false,
        ));
    }
    if !status.stdout.is_empty() {
        return Err(error(
            ErrorCode::ProjectGitDirty,
            "project Git worktree and index must be clean before adding a user message",
            false,
        ));
    }
    let after = git_head(path)?.ok_or_else(|| {
        error(
            ErrorCode::ProjectGitHeadUnavailable,
            "project repository HEAD disappeared while adding a user message",
            true,
        )
    })?;
    if before != after {
        return Err(error(
            ErrorCode::ProjectGitHeadUnavailable,
            "project repository HEAD changed while adding a user message; retry",
            true,
        ));
    }
    let index_after = git_index_tree(path)?;
    if index_before != index_after {
        return Err(error(
            ErrorCode::ProjectGitDirty,
            "project Git index changed while adding a user message; retry",
            true,
        ));
    }
    let head_tree = git_commit_tree(path, &after)?;
    if index_after != head_tree {
        return Err(error(
            ErrorCode::ProjectGitDirty,
            "project Git index does not match HEAD at write admission",
            false,
        ));
    }
    Ok(GitBaseline {
        commit: after,
        index_tree: index_after,
    })
}

fn git_index_tree(path: &Path) -> Result<String, ApiError> {
    let output = ProcessCommand::new("git")
        .arg("-C")
        .arg(path)
        .arg("write-tree")
        .output()
        .map_err(|failure| {
            error(
                ErrorCode::ProjectGitHeadUnavailable,
                format!("cannot snapshot Project Git index: {failure}"),
                false,
            )
        })?;
    if !output.status.success() {
        return Err(error(
            ErrorCode::ProjectGitDirty,
            String::from_utf8_lossy(&output.stderr).trim(),
            false,
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn git_commit_tree(path: &Path, commit: &str) -> Result<String, ApiError> {
    let expression = format!("{commit}^{{tree}}");
    let output = ProcessCommand::new("git")
        .arg("-C")
        .arg(path)
        .args(["rev-parse", "--verify", &expression])
        .output()
        .map_err(|failure| {
            error(
                ErrorCode::ProjectGitHeadUnavailable,
                format!("cannot resolve Project Git commit tree: {failure}"),
                false,
            )
        })?;
    if !output.status.success() {
        return Err(error(
            ErrorCode::ProjectGitHeadUnavailable,
            String::from_utf8_lossy(&output.stderr).trim(),
            false,
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn git_head(path: &Path) -> Result<Option<String>, ApiError> {
    let output = ProcessCommand::new("git")
        .arg("-C")
        .arg(path)
        .args(["rev-parse", "--verify", "HEAD"])
        .output()
        .map_err(|failure| {
            error(
                ErrorCode::ProjectGitHeadUnavailable,
                failure.to_string(),
                false,
            )
        })?;
    if !output.status.success() {
        return Ok(None);
    }
    let head = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if is_git_commit(&head) {
        Ok(Some(head))
    } else {
        Err(error(
            ErrorCode::ProjectGitHeadUnavailable,
            "Git returned an invalid full HEAD object id",
            false,
        ))
    }
}

fn is_git_commit(value: &str) -> bool {
    matches!(value.len(), 40 | 64)
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn git_top_level(path: &Path) -> Option<std::path::PathBuf> {
    let output = ProcessCommand::new("git")
        .arg("-C")
        .arg(path)
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Path::new(String::from_utf8_lossy(&output.stdout).trim())
        .canonicalize()
        .ok()
}

async fn wait_for_workspace_terminal_persistence(failures: &mut u32) {
    let exponent = (*failures).min(8);
    let delay_ms = 1_u64 << exponent;
    *failures = failures.saturating_add(1);
    tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
}

fn apply_workspace_terminal_result(
    state: &mut WorkingSet,
    run: &mut RunView,
    result: &Result<WorkspaceAgentResponse, DomainError>,
) {
    match result {
        Ok(output) => {
            let operations = output
                .operations
                .iter()
                .map(|operation| {
                    json!({
                        "id": operation.id,
                        "kind": operation.kind,
                        "status": operation.status,
                        "title": operation.title,
                        "summary": operation.summary,
                        "detail": operation.detail,
                        "paths": operation.paths,
                    })
                })
                .collect::<Vec<_>>();
            let output_items = output
                .output_items
                .iter()
                .map(|item| match item {
                    WorkspaceOutputItem::Message { id, phase, text } => json!({
                        "type": "message",
                        "id": id,
                        "phase": phase,
                        "text": text,
                    }),
                    WorkspaceOutputItem::Operation { id } => json!({
                        "type": "operation",
                        "id": id,
                    }),
                })
                .collect::<Vec<_>>();
            let data =
                (output.commit_id.is_some() || !operations.is_empty() || !output_items.is_empty())
                    .then(|| {
                        json!({"codex":{
                            "commit_id": output.commit_id,
                            "operations": operations,
                            "output_items": output_items,
                        }})
                    });
            let parent = run
                .last_message_id
                .as_deref()
                .unwrap_or(&run.base_message_id)
                .to_owned();
            let reply = message(
                &run.project_id,
                Some(&parent),
                "assistant",
                "standard",
                Some(output.assistant_text.clone()),
                None,
                data,
            );
            append_output(state, run, reply);
            run.status = "completed".into();
            run.error = None;
        }
        Err(failure) => {
            run.status = match failure.code {
                ErrorCode::RunCancelled => "cancelled",
                ErrorCode::RunLimitExceeded => "limit_exceeded",
                _ => "failed",
            }
            .into();
            run.error = Some(error(failure.code, &failure.message, failure.retryable));
        }
    }
}

fn pending<T: Serialize>(kind: &str, entity_id: Option<String>, body: &T) -> PendingEvent {
    PendingEvent {
        kind: kind.into(),
        entity_id,
        body: serde_json::to_value(body).unwrap_or(Value::Null),
        created_at: now(),
    }
}

fn now() -> i64 {
    i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
    )
    .unwrap_or(i64::MAX)
}
fn error(code: ErrorCode, message: impl Into<String>, retryable: bool) -> ApiError {
    ApiError {
        code,
        message: message.into(),
        retryable,
    }
}
#[allow(clippy::needless_pass_by_value)]
fn store_error(failure: ControlStoreError) -> ApiError {
    error(ErrorCode::RunRecoveryFailed, failure.to_string(), true)
}
fn record_value<'a>(read: &'a ControlRead, kind: ControlRecordKind, id: &str) -> Option<&'a Value> {
    read.records
        .iter()
        .find(|record| record.kind == kind && record.id == id)
        .map(|record| &record.value)
}
fn required_string(value: &Value, field: &str) -> Result<String, ApiError> {
    value
        .get(field)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| {
            error(
                ErrorCode::RunRecoveryFailed,
                format!("control record is missing {field}"),
                false,
            )
        })
}
fn agent_provider_id(agent: &Value) -> Result<String, ApiError> {
    agent
        .pointer("/config/provider_id")
        .and_then(Value::as_str)
        .or_else(|| {
            agent
                .get("mode")
                .and_then(Value::as_str)
                .map(|kind| match kind {
                    "codex" => "builtin-codex",
                    "openai" => "builtin-openai",
                    "deepseek" => "builtin-deepseek",
                    _ => kind,
                })
        })
        .map(str::to_owned)
        .ok_or_else(|| {
            error(
                ErrorCode::InvalidAgentConfiguration,
                "Agent provider reference is missing",
                false,
            )
        })
}
#[allow(clippy::needless_pass_by_value)]
fn serialization_error(failure: serde_json::Error) -> ApiError {
    error(ErrorCode::RunRecoveryFailed, failure.to_string(), false)
}
fn decode_records(read: ControlRead) -> Result<LoadedWorkingSet, ApiError> {
    let mut value = json!({
        "projects": [],
        "agents": [],
        "providers": [],
        "provider_credentials": {},
        "run_credentials": {},
        "sessions": [],
        "messages": [],
        "runs": [],
        "workspace_run_journals": {},
        "crons": [],
        "settings": default_settings(),
        "settings_revision": default_settings_revision(),
    });
    for record in read.records {
        match record.kind {
            ControlRecordKind::Project => value["projects"]
                .as_array_mut()
                .expect("record array")
                .push(record.value),
            ControlRecordKind::Agent => value["agents"]
                .as_array_mut()
                .expect("record array")
                .push(record.value),
            ControlRecordKind::Provider => value["providers"]
                .as_array_mut()
                .expect("record array")
                .push(record.value),
            ControlRecordKind::ProviderCredential => {
                value["provider_credentials"][record.id] = record.value;
            }
            ControlRecordKind::RunCredential => {
                value["run_credentials"][record.id] = record.value;
            }
            ControlRecordKind::Session => value["sessions"]
                .as_array_mut()
                .expect("record array")
                .push(record.value),
            ControlRecordKind::Message => value["messages"]
                .as_array_mut()
                .expect("record array")
                .push(record.value),
            ControlRecordKind::Run => value["runs"]
                .as_array_mut()
                .expect("record array")
                .push(record.value),
            ControlRecordKind::WorkspaceRunJournal => {
                value["workspace_run_journals"][record.id] = record.value;
            }
            ControlRecordKind::Cron => value["crons"]
                .as_array_mut()
                .expect("record array")
                .push(record.value),
            ControlRecordKind::Settings => {
                value["settings"] = record.value["values"].clone();
                value["settings_revision"] = record.value["revision"].clone();
            }
        }
    }
    if value["agents"]
        .as_array()
        .into_iter()
        .flatten()
        .any(|agent| agent.get("config").is_none())
    {
        value
            .as_object_mut()
            .expect("record object")
            .remove("providers");
    }
    value = migrate_state(value)?;
    let mut state: WorkingSet = serde_json::from_value(value).map_err(serialization_error)?;
    for provider in builtin_providers() {
        if !state
            .providers
            .iter()
            .any(|existing| existing.provider.id == provider.provider.id)
        {
            state.providers.push(provider);
        }
    }
    state.settings.0.retain(|id, _| !id.starts_with("models."));
    Ok(LoadedWorkingSet {
        revision: read.revision,
        original: state,
    })
}

fn record_changes(
    original: &WorkingSet,
    updated: &WorkingSet,
) -> Result<Vec<ControlChange>, ApiError> {
    let original = encode_records(original)?;
    let updated = encode_records(updated)?;
    let mut changes = Vec::new();
    for (key, record) in &updated {
        if original.get(key) != Some(record) {
            changes.push(ControlChange::Put(record.clone()));
        }
    }
    for ((kind, id), _) in original {
        if !updated.contains_key(&(kind, id.clone())) {
            changes.push(ControlChange::Delete { kind, id });
        }
    }
    Ok(changes)
}

#[allow(clippy::too_many_lines)]
fn encode_records(
    state: &WorkingSet,
) -> Result<BTreeMap<(ControlRecordKind, String), ControlRecord>, ApiError> {
    let mut records = BTreeMap::new();
    let mut insert = |kind, id: String, project_id: Option<String>, value| {
        records.insert(
            (kind, id.clone()),
            ControlRecord {
                kind,
                id,
                project_id,
                value,
            },
        );
    };
    for project in &state.projects {
        insert(
            ControlRecordKind::Project,
            project.id.clone(),
            Some(project.id.clone()),
            serde_json::to_value(project).map_err(serialization_error)?,
        );
    }
    for agent in &state.agents {
        insert(
            ControlRecordKind::Agent,
            agent.id.clone(),
            None,
            serde_json::to_value(agent).map_err(serialization_error)?,
        );
    }
    for provider in &state.providers {
        insert(
            ControlRecordKind::Provider,
            provider.provider.id.clone(),
            None,
            serde_json::to_value(provider).map_err(serialization_error)?,
        );
    }
    for (id, reference) in &state.provider_credentials {
        insert(
            ControlRecordKind::ProviderCredential,
            id.clone(),
            None,
            json!(reference),
        );
    }
    let run_projects = state
        .runs
        .iter()
        .map(|run| (run.id.as_str(), run.project_id.as_str()))
        .collect::<HashMap<_, _>>();
    for (id, reference) in &state.run_credentials {
        insert(
            ControlRecordKind::RunCredential,
            id.clone(),
            run_projects.get(id.as_str()).map(|id| (*id).to_owned()),
            json!(reference),
        );
    }
    for session in &state.sessions {
        insert(
            ControlRecordKind::Session,
            session.id.clone(),
            Some(session.project_id.clone()),
            serde_json::to_value(session).map_err(serialization_error)?,
        );
    }
    for message in &state.messages {
        insert(
            ControlRecordKind::Message,
            message.id.clone(),
            Some(message.project_id.clone()),
            serde_json::to_value(message).map_err(serialization_error)?,
        );
    }
    for run in &state.runs {
        insert(
            ControlRecordKind::Run,
            run.id.clone(),
            Some(run.project_id.clone()),
            serde_json::to_value(run).map_err(serialization_error)?,
        );
    }
    for (id, journal) in &state.workspace_run_journals {
        insert(
            ControlRecordKind::WorkspaceRunJournal,
            id.clone(),
            run_projects.get(id.as_str()).map(|id| (*id).to_owned()),
            serde_json::to_value(journal).map_err(serialization_error)?,
        );
    }
    for cron in &state.crons {
        insert(
            ControlRecordKind::Cron,
            cron.id.clone(),
            Some(cron.project_id.clone()),
            serde_json::to_value(cron).map_err(serialization_error)?,
        );
    }
    insert(
        ControlRecordKind::Settings,
        "settings".into(),
        None,
        json!({"values": state.settings, "revision": state.settings_revision}),
    );
    Ok(records)
}

fn validate_archive_catalog(archive: &ProjectExport) -> Result<(), ApiError> {
    let mut provider_ids = HashSet::new();
    for provider in &archive.providers {
        validate_provider(provider)?;
        #[cfg(all(feature = "dev-mock-provider", debug_assertions))]
        if provider.kind == AgentMode::Mock {
            return Err(invalid_archive(
                "development Mock providers cannot be imported or exported",
            ));
        }
        if !provider_ids.insert(&provider.id) {
            return Err(invalid_archive("duplicate provider"));
        }
    }
    for agent in &archive.agents {
        // Archives preserve configuration even when a provider has delisted its model.
        // Availability is checked when starting new work, not when copying history.
        if !provider_ids.contains(&agent.config.provider_id)
            || agent.config.model.trim().is_empty()
            || agent
                .config
                .reasoning_effort
                .as_ref()
                .is_some_and(|effort| effort.trim().is_empty())
        {
            return Err(invalid_archive(
                "invalid archived Agent configuration or provider reference",
            ));
        }
        if let Some(owner) = &agent.owner_session_id {
            if !agent.name.is_empty()
                || !archive
                    .sessions
                    .iter()
                    .any(|s| &s.id == owner && s.agent_id == agent.id)
            {
                return Err(invalid_archive("invalid anonymous Agent owner"));
            }
        } else if agent.name.trim().is_empty() {
            return Err(invalid_archive("named Agent requires a name"));
        }
    }
    if archive.project.default_agent_id.as_ref().is_some_and(|id| {
        archive
            .agents
            .iter()
            .any(|a| &a.id == id && a.owner_session_id.is_some())
    }) {
        return Err(invalid_archive(
            "Project default Agent must be a named preset",
        ));
    }
    for session in &archive.sessions {
        if archive.agents.iter().any(|a| {
            a.id == session.agent_id
                && a.owner_session_id
                    .as_ref()
                    .is_some_and(|owner| owner != &session.id)
        }) {
            return Err(invalid_archive(
                "anonymous Agent cannot be shared between Sessions",
            ));
        }
    }
    Ok(())
}
