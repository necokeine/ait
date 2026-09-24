//! Authenticated local HTTP and WebSocket transport for the independent server.

mod agent_execution;
mod agent_runtime;
mod agents;
mod auth;
mod checkout;
mod connection;
mod daemon;
mod directory;
mod files;
mod forge;
mod github_projects;
mod jobs;
mod outbound;
mod workspace_automation;
mod workspace_labels;
mod workspace_recovery;
mod workspace_state;
mod worktrees;

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::{Request, State, WebSocketUpgrade};
use axum::http::StatusCode;
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use secrecy::SecretString;
use server_filesystem::service::checkout::Checkout;
use server_filesystem::service::files::Files;
use server_filesystem::service::forge::Forge;
use server_filesystem::service::github_projects::GithubProjects;
use server_filesystem::service::workspace_recovery::WorkspaceRecovery;
use server_filesystem::service::worktrees::Worktrees;
use server_metadata::service::daemon::Daemon;
use server_metadata::service::directory::Directory;
use server_metadata::service::workspace_automation::WorkspaceAutomation;
use server_metadata::service::workspace_labels::WorkspaceLabels;
use server_metadata::service::workspace_state::WorkspaceState;
use server_protocol::{CAPABILITIES, Lifecycle, Limits, ServerInfo, VERSION};
use server_provider::service::agent_execution::AgentExecution;
use server_provider::service::agent_runtime::AgentRuntimeDirectory;
use server_provider::service::agents::Agents;
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;
use tower_http::timeout::TimeoutLayer;
use tower_http::trace::TraceLayer;

pub use auth::validate_token;

/// Invalid startup configuration for the transport.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// Only loopback addresses with an assigned port are supported.
    #[error("server requires a loopback address with an assigned port")]
    InvalidAddress,
    /// Tokens must contain 32–256 visible ASCII characters without spaces.
    #[error("AIT_SERVER_TOKEN must contain 32–256 visible ASCII characters without spaces")]
    InvalidToken,
}

#[derive(Debug)]
struct Shared {
    info: ServerInfo,
    token: SecretString,
    authorities: Vec<String>,
    cancellation: CancellationToken,
    tasks: TaskTracker,
    connections: Arc<Semaphore>,
    // Serializes admission with closing the tracker, including pending HTTP upgrades.
    admission: Mutex<()>,
    lifecycle_intent: Mutex<Option<LifecycleIntent>>,
    agents: Option<Arc<Mutex<Agents>>>,
    checkout: Option<Arc<Mutex<Checkout>>>,
    agent_runtime: Option<Arc<Mutex<AgentRuntimeDirectory>>>,
    agent_execution: Option<AgentExecution>,
    execution_waits: Arc<Semaphore>,
    daemon: Option<Arc<Mutex<Daemon>>>,
    directory: Option<Arc<Mutex<Directory>>>,
    forge: Option<Arc<Mutex<Forge>>>,
    files: Option<Arc<Mutex<Files>>>,
    workspace_labels: Option<Arc<Mutex<WorkspaceLabels>>>,
    workspace_automation: Option<Arc<Mutex<WorkspaceAutomation>>>,
    workspace_state: Option<Arc<Mutex<WorkspaceState>>>,
    worktrees: Option<Arc<Mutex<Worktrees>>>,
    github_projects: Option<Arc<Mutex<GithubProjects>>>,
    workspace_recovery: Option<Arc<Mutex<WorkspaceRecovery>>>,
    jobs: Arc<Semaphore>,
}

/// Optional independently composed business services; only installed methods are advertised.
#[derive(Debug, Default)]
pub struct Services {
    /// Native Provider execution and coordinated Agent runtime metadata.
    pub agent_execution: Option<AgentExecution>,
    /// Filesystem workspace recovery operations.
    pub workspace_recovery: Option<WorkspaceRecovery>,
    /// Filesystem github projects operations.
    pub github_projects: Option<GithubProjects>,
    /// Versioned Agent presets and explicit default selection.
    pub agents: Option<Agents>,
    /// Git checkout status, diff, refresh, and history use cases.
    pub checkout: Option<Checkout>,
    /// Paseo Agent runtime directory and metadata lifecycle use cases.
    pub agent_runtime: Option<AgentRuntimeDirectory>,
    /// Daemon status, mutable configuration, diagnostics, and update boundary.
    pub daemon: Option<Daemon>,
    /// Paseo-shaped project and workspace registries.
    pub directory: Option<Directory>,
    /// Forge search and pull request use cases.
    pub forge: Option<Forge>,
    /// Scoped filesystem, upload, and download operations.
    pub files: Option<Files>,
    /// Paseo workspace label catalog, assignment, and subscription use cases.
    pub workspace_labels: Option<WorkspaceLabels>,
    /// Paseo workspace setup and configured script runtime.
    pub workspace_automation: Option<WorkspaceAutomation>,
    /// Workspace attention and archived-placement recovery use cases.
    pub workspace_state: Option<WorkspaceState>,
    /// Paseo-owned Git worktree lifecycle use cases.
    pub worktrees: Option<Worktrees>,
}

pub use server_metadata::rpc::daemon::LifecycleIntent;

impl Shared {
    fn info(&self) -> ServerInfo {
        let mut info = self.info.clone();
        if self.cancellation.is_cancelled() {
            info.lifecycle = Lifecycle::Draining;
        }
        info
    }

    fn request_lifecycle(&self, intent: LifecycleIntent) {
        let admission = self
            .admission
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut requested = self
            .lifecycle_intent
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if requested.is_none() {
            *requested = Some(intent);
        }
        self.cancellation.cancel();
        self.tasks.close();
        drop(requested);
        drop(admission);
    }
}

/// Cloneable transport owner; the process host owns the listener and shutdown deadline.
#[derive(Debug, Clone)]
pub struct Api {
    shared: Arc<Shared>,
}

impl Api {
    /// Construct transport state for an already-bound loopback listener.
    ///
    /// `server_id` is persisted by the host; `instance_id` is unique to this process start.
    /// `token` authenticates the info and upgrade endpoints and is never returned to clients.
    /// `services` selects implemented methods; catalog placeholders remain negotiable.
    ///
    /// # Errors
    /// Returns an error for non-loopback/unassigned addresses or invalid tokens.
    pub fn new(
        address: SocketAddr,
        server_id: String,
        instance_id: String,
        token: SecretString,
        services: Services,
    ) -> Result<Self, ConfigError> {
        use secrecy::ExposeSecret;
        if !address.ip().is_loopback() || address.port() == 0 {
            return Err(ConfigError::InvalidAddress);
        }
        validate_token(token.expose_secret())?;
        let mut authorities = vec![address.to_string(), format!("localhost:{}", address.port())];
        if address.port() == 80 {
            // HTTP clients omit the default port in Host and Origin.
            authorities.push(address.to_string().trim_end_matches(":80").to_owned());
            authorities.push("localhost".to_owned());
        }
        let implemented_capabilities = installed_capabilities(&services);
        let capabilities = registered_capabilities(&implemented_capabilities);
        Ok(Self {
            shared: Arc::new(Shared {
                info: ServerInfo {
                    server_id,
                    instance_id,
                    listen: address.to_string(),
                    lifecycle: Lifecycle::Ready,
                    protocol: VERSION,
                    capabilities,
                    implemented_capabilities,
                    limits: Limits::default(),
                },
                token,
                authorities,
                cancellation: CancellationToken::new(),
                tasks: TaskTracker::new(),
                connections: Arc::new(Semaphore::new(server_protocol::MAX_CONNECTIONS)),
                admission: Mutex::new(()),
                lifecycle_intent: Mutex::new(None),
                agents: services.agents.map(|agents| Arc::new(Mutex::new(agents))),
                checkout: services
                    .checkout
                    .map(|checkout| Arc::new(Mutex::new(checkout))),
                agent_runtime: services
                    .agent_runtime
                    .map(|directory| Arc::new(Mutex::new(directory))),
                agent_execution: services.agent_execution,
                execution_waits: Arc::new(Semaphore::new(32)),
                daemon: services.daemon.map(|daemon| Arc::new(Mutex::new(daemon))),
                directory: services
                    .directory
                    .map(|directory| Arc::new(Mutex::new(directory))),
                forge: services.forge.map(|forge| Arc::new(Mutex::new(forge))),
                files: services.files.map(|files| Arc::new(Mutex::new(files))),
                workspace_labels: services
                    .workspace_labels
                    .map(|labels| Arc::new(Mutex::new(labels))),
                workspace_automation: services
                    .workspace_automation
                    .map(|automation| Arc::new(Mutex::new(automation))),
                github_projects: services
                    .github_projects
                    .map(|service| Arc::new(Mutex::new(service))),
                workspace_recovery: services
                    .workspace_recovery
                    .map(|service| Arc::new(Mutex::new(service))),
                workspace_state: services
                    .workspace_state
                    .map(|workspace_state| Arc::new(Mutex::new(workspace_state))),
                worktrees: services
                    .worktrees
                    .map(|worktrees| Arc::new(Mutex::new(worktrees))),
                jobs: Arc::new(Semaphore::new(1)),
            }),
        })
    }

    /// Build routes with origin checks, bounded HTTP handling, and metadata-only tracing.
    pub fn router(&self) -> Router {
        Router::new()
            .route("/healthz", get(health))
            .route("/readyz", get(ready))
            .route("/v1/server/info", get(info))
            .route("/v1/ws", get(upgrade))
            .route("/api/files/download", get(files::download))
            .fallback(|| async { ApiError(StatusCode::NOT_FOUND) })
            .layer(middleware::from_fn_with_state(self.shared.clone(), guard))
            .layer(TimeoutLayer::with_status_code(
                StatusCode::REQUEST_TIMEOUT,
                Duration::from_secs(10),
            ))
            .layer(TraceLayer::new_for_http().make_span_with(
                |request: &Request| tracing::info_span!("server_http", method = %request.method()),
            ))
            .with_state(self.shared.clone())
    }

    /// Atomically reject new upgrades, change readiness, and signal existing connections.
    pub fn begin_shutdown(&self) {
        let _guard = self
            .shared
            .admission
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.shared.cancellation.cancel();
        self.shared.tasks.close();
    }

    /// Return the first WebSocket lifecycle request accepted during this run.
    #[must_use]
    pub fn lifecycle_intent(&self) -> Option<LifecycleIntent> {
        self.shared
            .lifecycle_intent
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Wait for all admitted upgrades and connections after calling `begin_shutdown`.
    /// The process host must bound this wait with its shutdown deadline.
    pub async fn wait_closed(&self) {
        self.shared.tasks.wait().await;
        if let Some(execution) = &self.shared.agent_execution
            && execution.shutdown().await.is_err()
        {
            tracing::error!("native Provider shutdown failed");
        }
    }

    /// Wait until shutdown has closed admission; useful for bounding the host's drain phase.
    pub async fn wait_draining(&self) {
        self.shared.cancellation.cancelled().await;
    }
}

fn installed_capabilities(services: &Services) -> Vec<String> {
    let mut capabilities: Vec<String> = CAPABILITIES.iter().map(|s| (*s).to_owned()).collect();
    let groups: &[(bool, &[&str])] = &[
        (
            services.files.is_some(),
            server_filesystem::protocol::files::CAPABILITIES,
        ),
        (
            services.agents.is_some(),
            server_provider::protocol::agent::CAPABILITIES,
        ),
        (
            services.agent_runtime.is_some() || services.agent_execution.is_some(),
            server_provider::protocol::agent_lifecycle::CAPABILITIES,
        ),
        (
            services.agent_execution.is_some(),
            server_provider::protocol::agent_execution::CAPABILITIES,
        ),
        (
            services.checkout.is_some(),
            server_filesystem::protocol::checkout::CAPABILITIES,
        ),
        (
            services.daemon.is_some(),
            server_metadata::protocol::daemon::CAPABILITIES,
        ),
        (
            services.directory.is_some(),
            server_metadata::protocol::directory::CAPABILITIES,
        ),
        (
            services.github_projects.is_some(),
            server_filesystem::protocol::github_projects::CAPABILITIES,
        ),
        (
            services.directory.is_some(),
            server_metadata::protocol::project_config::CAPABILITIES,
        ),
        (
            services.directory.is_some(),
            server_metadata::protocol::project_icon::CAPABILITIES,
        ),
        (
            services.forge.is_some(),
            server_filesystem::protocol::forge::CAPABILITIES,
        ),
        (
            services.workspace_labels.is_some(),
            server_metadata::protocol::workspace_labels::CAPABILITIES,
        ),
        (
            services.worktrees.is_some(),
            server_filesystem::protocol::worktrees::CAPABILITIES,
        ),
        (
            services.workspace_automation.is_some(),
            server_metadata::protocol::workspace_automation::CAPABILITIES,
        ),
        (
            services.workspace_recovery.is_some(),
            server_filesystem::protocol::workspace_recovery::CAPABILITIES,
        ),
        (
            services.workspace_state.is_some(),
            server_metadata::protocol::workspace_state::CAPABILITIES,
        ),
    ];
    for (_, group) in groups.iter().filter(|(installed, _)| *installed) {
        capabilities.extend(group.iter().map(|method| (*method).to_owned()));
    }
    capabilities
}

fn registered_capabilities(implemented: &[String]) -> Vec<String> {
    let mut capabilities = implemented.to_vec();
    for method in server_protocol::methods::PASEO_METHODS {
        if !capabilities
            .iter()
            .any(|capability| capability == method.canonical_name)
        {
            capabilities.push(method.canonical_name.to_owned());
        }
    }
    capabilities
}

struct ApiError(StatusCode);

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.0,
            Json(
                serde_json::json!({"error": self.0.canonical_reason().unwrap_or("request failed")}),
            ),
        )
            .into_response()
    }
}

async fn guard(
    State(state): State<Arc<Shared>>,
    request: Request,
    next: Next,
) -> Result<Response, ApiError> {
    auth::validate_source(request.headers(), &state.authorities)?;
    let download = request.method() == axum::http::Method::GET
        && request.uri().path() == "/api/files/download";
    if !matches!(request.uri().path(), "/healthz" | "/readyz") && !download {
        auth::authenticate(request.headers(), &state.token)?;
    }
    // Credentials and client state must never be accepted in a URL.
    if request.uri().query().is_some() && !download {
        return Err(ApiError(StatusCode::BAD_REQUEST));
    }
    Ok(next.run(request).await)
}

async fn health() -> Result<Response, ApiError> {
    Ok(Json(serde_json::json!({"status":"alive"})).into_response())
}

async fn ready(State(state): State<Arc<Shared>>) -> Result<Response, ApiError> {
    if state.cancellation.is_cancelled() {
        return Err(ApiError(StatusCode::SERVICE_UNAVAILABLE));
    }
    Ok(Json(serde_json::json!({"status":"ready"})).into_response())
}

async fn info(State(state): State<Arc<Shared>>) -> Result<Response, ApiError> {
    Ok(Json(state.info()).into_response())
}

async fn upgrade(
    State(state): State<Arc<Shared>>,
    ws: WebSocketUpgrade,
) -> Result<Response, ApiError> {
    let guard = state
        .admission
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if state.cancellation.is_cancelled() {
        return Err(ApiError(StatusCode::SERVICE_UNAVAILABLE));
    }
    let permit = state
        .connections
        .clone()
        .try_acquire_owned()
        .map_err(|_| ApiError(StatusCode::TOO_MANY_REQUESTS))?;
    let tracking = state.tasks.token();
    drop(guard);
    Ok(ws
        .max_message_size(server_protocol::MAX_MESSAGE_BYTES)
        .max_frame_size(server_protocol::MAX_MESSAGE_BYTES)
        .write_buffer_size(0)
        .max_write_buffer_size(server_protocol::MAX_QUEUE_BYTES)
        .on_upgrade(move |socket| async move {
            let (_tracking, _permit) = (tracking, permit);
            connection::serve(socket, state).await;
        }))
}

#[cfg(test)]
mod tests;
