//! Shared service composition, public command entry points and exhaustive command routing.
mod model;

use crate::control::runs::finalization::RunControl;
use ait_contracts::{Command, Response};
use ait_ports::{
    AgentProviderGateway, ControlStore, HostProviderModelCatalog, ProjectDirectoryCreator,
    SessionTitleGenerator, WorkspaceAgent, WorkspaceApprovalDecision,
};
use std::collections::HashMap;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, Weak};

pub(in crate::control) mod admission;
pub(in crate::control) mod approvals;
pub(in crate::control) mod catalog;
pub(in crate::control) mod conversation;
pub(in crate::control) mod cron;
pub(in crate::control) mod errors;
pub(in crate::control) mod events;
pub(in crate::control) mod execution;
pub(in crate::control) mod permissions;
pub(in crate::control) mod project;
pub(in crate::control) mod runs;
pub(in crate::control) mod settings;
pub(in crate::control) mod state;
pub(in crate::control) mod tool_approvals;
pub(in crate::control) mod tool_interactions;

pub use permissions::PermissionPolicyLimits;
pub use runs::recovery::StartupRecoveryPlan;

/// Shared application entry point used by every transport adapter.
#[derive(Clone)]
pub struct LocalControlService {
    store: Arc<dyn ControlStore>,
    project_directory_creator: Option<Arc<dyn ProjectDirectoryCreator>>,
    session_leases: Arc<Mutex<HashMap<String, Weak<()>>>>,
    project_workspace: Arc<dyn ait_ports::ProjectWorkspace>,
    cancellations: Arc<Mutex<HashMap<String, tokio_util::sync::CancellationToken>>>,
    run_controls: Arc<Mutex<HashMap<String, Weak<RunControl>>>>,
    approval_waiters:
        Arc<Mutex<HashMap<String, tokio::sync::watch::Sender<Option<WorkspaceApprovalDecision>>>>>,
    tool_approval_timeout: std::time::Duration,
    permission_limits: PermissionPolicyLimits,
    tool_approval_waiters: Arc<Mutex<HashMap<String, tool_approvals::ToolApprovalWaiter>>>,
    tool_interaction_waiters: Arc<Mutex<HashMap<String, tool_interactions::ToolInteractionWaiter>>>,
    provider_gateway: Option<Arc<dyn AgentProviderGateway>>,
    api_tools: Option<Arc<dyn ait_ports::RunToolFactory>>,
    run_dispatcher: Option<Arc<dyn ait_ports::RunDispatcher>>,
    draining: Arc<AtomicBool>,
    admission: Arc<tokio::sync::RwLock<()>>,
    host_provider_catalog: Option<Arc<dyn HostProviderModelCatalog>>,
    workspace_agent: Option<Arc<dyn WorkspaceAgent>>,
    session_title_generator: Option<Arc<dyn SessionTitleGenerator>>,
}

impl LocalControlService {
    /// Set a shorter decision window; never extends the worker or Run budget.
    #[must_use]
    pub fn with_tool_approval_timeout(mut self, timeout: std::time::Duration) -> Self {
        self.tool_approval_timeout = timeout.clamp(
            std::time::Duration::from_millis(1),
            std::time::Duration::from_mins(2),
        );
        self
    }

    #[must_use]
    /// Creates a control service backed by the supplied workspace and store ports.
    pub fn new(
        project_workspace: Arc<dyn ait_ports::ProjectWorkspace>,
        store: Arc<dyn ControlStore>,
    ) -> Self {
        Self {
            store,
            project_directory_creator: None,
            session_leases: Arc::new(Mutex::new(HashMap::new())),
            project_workspace,
            cancellations: Arc::new(Mutex::new(HashMap::new())),
            run_controls: Arc::new(Mutex::new(HashMap::new())),
            approval_waiters: Arc::new(Mutex::new(HashMap::new())),
            tool_approval_timeout: std::time::Duration::from_mins(2),
            permission_limits: PermissionPolicyLimits::default(),
            tool_approval_waiters: Arc::new(Mutex::new(HashMap::new())),
            tool_interaction_waiters: Arc::new(Mutex::new(HashMap::new())),
            provider_gateway: None,
            api_tools: None,
            run_dispatcher: None,
            draining: Arc::new(AtomicBool::new(false)),
            admission: Arc::new(tokio::sync::RwLock::new(())),
            host_provider_catalog: None,
            workspace_agent: None,
            session_title_generator: None,
        }
    }

    /// Creates a service that can execute real workspace-scoped coding Agents.
    #[must_use]
    pub fn with_workspace_agent(
        project_workspace: Arc<dyn ait_ports::ProjectWorkspace>,
        store: Arc<dyn ControlStore>,
        workspace_agent: Arc<dyn WorkspaceAgent>,
    ) -> Self {
        Self {
            store,
            project_directory_creator: None,
            session_leases: Arc::new(Mutex::new(HashMap::new())),
            project_workspace,
            cancellations: Arc::new(Mutex::new(HashMap::new())),
            run_controls: Arc::new(Mutex::new(HashMap::new())),
            approval_waiters: Arc::new(Mutex::new(HashMap::new())),
            tool_approval_timeout: std::time::Duration::from_mins(2),
            permission_limits: PermissionPolicyLimits::default(),
            tool_approval_waiters: Arc::new(Mutex::new(HashMap::new())),
            tool_interaction_waiters: Arc::new(Mutex::new(HashMap::new())),
            provider_gateway: None,
            api_tools: None,
            run_dispatcher: None,
            draining: Arc::new(AtomicBool::new(false)),
            admission: Arc::new(tokio::sync::RwLock::new(())),
            host_provider_catalog: None,
            workspace_agent: Some(workspace_agent),
            session_title_generator: None,
        }
    }

    /// Installs the host tool executor factory used only by API Providers.
    #[must_use]
    pub fn with_api_tools(mut self, factory: Arc<dyn ait_ports::RunToolFactory>) -> Self {
        self.api_tools = Some(factory);
        self
    }

    /// Installs supervised execution for all API Runs, including startup recovery.
    #[must_use]
    pub fn with_run_dispatcher(mut self, dispatcher: Arc<dyn ait_ports::RunDispatcher>) -> Self {
        self.run_dispatcher = Some(dispatcher);
        self
    }

    /// Adds the host capability used only when Project registration omits a workdir.
    #[must_use]
    pub fn with_project_directory_creator(
        mut self,
        creator: Arc<dyn ProjectDirectoryCreator>,
    ) -> Self {
        self.project_directory_creator = Some(creator);
        self
    }

    /// Installs the credential and API completion gateway.
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
}

#[cfg(test)]
mod workspace_tests;
