//! Authenticated local HTTP and WebSocket transport for the independent server.

mod auth;
mod browser_auth;
mod capabilities;
mod connection;
mod files;
mod outbound;

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use server_model::Runtime;
use std::time::Duration;

use axum::extract::{Request, State, WebSocketUpgrade};
use axum::http::{HeaderMap, Method, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
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
use server_protocol::{Lifecycle, Limits, ServerInfo, VERSION};
use server_provider::service::agent_execution::AgentExecution;
use server_provider::service::agent_runtime::AgentRuntimeDirectory;
use server_provider::service::agents::Agents;
use tokio::sync::Semaphore;
use tower_http::timeout::TimeoutLayer;
use tower_http::trace::TraceLayer;

pub use auth::validate_token;
pub use browser_auth::validate_browser_origin;
use capabilities::installed_capabilities;

/// Invalid startup configuration for the transport.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// Only loopback addresses with an assigned port are supported.
    #[error("server requires a loopback address with an assigned port")]
    InvalidAddress,
    /// Tokens must contain 32–256 visible ASCII characters without spaces.
    #[error("AIT_SERVER_TOKEN must contain 32–256 visible ASCII characters without spaces")]
    InvalidToken,
    /// Browser origins must be explicit canonical HTTP loopback origins.
    #[error(
        "browser origin must be an HTTP loopback origin without a path (for example http://localhost:8081)"
    )]
    InvalidBrowserOrigin,
    /// Browser policy must be configured before cloning or serving the API.
    #[error("configure browser origins before cloning the API")]
    SharedConfiguration,
}

#[derive(Debug)]
struct Shared {
    runtime: Arc<Runtime>,
    token: SecretString,
    browser_auth: browser_auth::BrowserAuth,
    authorities: Vec<String>,
    connections: Arc<Semaphore>,
    metadata: Arc<server_metadata::dispatch::State>,
    filesystem: Arc<server_filesystem::dispatch::State>,
    provider: Arc<server_provider::dispatch::State>,
    terminal: Arc<server_terminal::dispatch::State>,
    voice: Arc<server_voice::dispatch::State>,
    schedule: server_schedule::dispatch::State,
    browser: server_browser::dispatch::State,
}

impl std::ops::Deref for Shared {
    type Target = Runtime;

    fn deref(&self) -> &Runtime {
        &self.runtime
    }
}

/// Optional independently composed business services; only installed methods are advertised.
#[derive(Debug, Default)]
pub struct Services {
    /// Persistent timed Agent executions.
    pub schedules: Option<server_schedule::service::Schedules>,
    /// Connection-owned browser automation broker.
    pub browser: Option<server_browser::broker::Broker>,
    /// Orchestration skill selection and installation.
    pub skills: Option<server_filesystem::service::skills::Skills>,
    /// Durable push registration and lease renewal.
    pub push_tokens: Option<server_metadata::service::push::PushTokens>,
    /// Connection-owned voice and dictation with independently selected speech engines.
    pub speech: Option<server_voice::service::Speech>,
    /// Local PTY terminal lifecycle, input, capture, and streaming.
    pub terminals: Option<server_terminal::service::Terminals>,
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
    fn start_draining(&self) {
        if let Some(schedules) = &self.schedule.schedules {
            schedules.stop();
        }
        self.metadata.start_draining();
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
        let implemented_capabilities = installed_capabilities(&services);
        let capabilities = registered_capabilities(&implemented_capabilities);
        let session_events = services
            .agent_execution
            .as_ref()
            .map(AgentExecution::events)
            .unwrap_or_default();
        let creations = services
            .agent_execution
            .as_ref()
            .map(AgentExecution::creations)
            .or_else(|| services.directory.as_ref().map(Directory::creations))
            .unwrap_or_default();
        let runtime = Arc::new(Runtime::new(ServerInfo {
            server_id,
            instance_id,
            listen: address.to_string(),
            lifecycle: Lifecycle::Ready,
            protocol: VERSION,
            capabilities,
            implemented_capabilities,
            limits: Limits::default(),
        }));
        let metadata = Arc::new(server_metadata::dispatch::State {
            push_tokens: services.push_tokens.map(shared_service),
            runtime: runtime.clone(),
            daemon: services.daemon.map(shared_service),
            directory: services.directory.map(shared_service),
            workspace_labels: services.workspace_labels.map(shared_service),
            workspace_automation: services.workspace_automation.map(shared_service),
            workspace_state: services.workspace_state.map(shared_service),
            session_events,
            creations,
            has_agent_execution: services.agent_execution.is_some(),
        });
        let filesystem = Arc::new(server_filesystem::dispatch::State {
            runtime: runtime.clone(),
            checkout: services.checkout.map(shared_service),
            forge: services.forge.map(shared_service),
            files: services.files.map(shared_service),
            github_projects: services.github_projects.map(shared_service),
            worktrees: services.worktrees.map(shared_service),
            workspace_recovery: services.workspace_recovery.map(shared_service),
            skills: services.skills.map(shared_service),
            workspace_automation: metadata.workspace_automation.clone(),
        });
        let provider = Arc::new(server_provider::dispatch::State {
            runtime: runtime.clone(),
            agents: services.agents.map(shared_service),
            agent_runtime: services.agent_runtime.map(shared_service),
            agent_execution: services.agent_execution,
            has_terminals: services.terminals.is_some(),
        });
        let terminal = Arc::new(server_terminal::dispatch::State {
            runtime: runtime.clone(),
            terminals: services.terminals.map(shared_service),
        });
        let api = Self {
            shared: Arc::new(Shared {
                schedule: server_schedule::dispatch::State {
                    schedules: services.schedules,
                },
                browser: server_browser::dispatch::State {
                    broker: services.browser,
                },
                voice: Arc::new(server_voice::dispatch::State {
                    runtime: runtime.clone(),
                    speech: services.speech,
                }),
                runtime,
                metadata,
                filesystem,
                provider,
                terminal,
                token,
                browser_auth: browser_auth::BrowserAuth::default(),
                authorities: allowed_authorities(address),
                connections: Arc::new(Semaphore::new(server_protocol::MAX_CONNECTIONS)),
            }),
        };
        server_terminal::connection::maintain(&api.shared.terminal);
        Ok(api)
    }

    /// Allow browser pages from the given explicit HTTP loopback origins.
    ///
    /// Returns the configured API; configure it before cloning or serving it.
    /// # Errors
    /// Rejects invalid origins or an API that has already been cloned.
    pub fn with_browser_origins(mut self, origins: Vec<String>) -> Result<Self, ConfigError> {
        let browser_auth = browser_auth::BrowserAuth::new(origins)?;
        Arc::get_mut(&mut self.shared)
            .ok_or(ConfigError::SharedConfiguration)?
            .browser_auth = browser_auth;
        Ok(self)
    }

    /// Return the composed broker for host-side browser tool execution.
    #[must_use]
    pub fn browser(&self) -> Option<server_browser::broker::Broker> {
        self.shared.browser.broker.clone()
    }

    /// Build routes with origin checks, bounded HTTP handling, and metadata-only tracing.
    pub fn router(&self) -> Router {
        Router::new()
            .route("/healthz", get(health))
            .route("/readyz", get(ready))
            .route("/v1/server/info", get(info))
            .route("/v1/ws", get(upgrade))
            .route(browser_auth::TICKET_PATH, post(browser_ticket))
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
        self.shared.start_draining();
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
        if let Some(schedules) = &self.shared.schedule.schedules
            && schedules.shutdown().await.is_err()
        {
            tracing::error!("schedule shutdown failed");
        }
        if let Some(terminals) = self.shared.terminal.terminals.clone() {
            let result = tokio::task::spawn_blocking(move || {
                terminals
                    .lock()
                    .map_err(|_| server_terminal::Error::Io)?
                    .shutdown()
            })
            .await;
            if !matches!(result, Ok(Ok(()))) {
                tracing::error!("terminal shutdown failed");
            }
        }
        if let Some(execution) = &self.shared.provider.agent_execution
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

fn allowed_authorities(address: SocketAddr) -> Vec<String> {
    let mut authorities = vec![address.to_string(), format!("localhost:{}", address.port())];
    if address.port() == 80 {
        // HTTP clients omit the default port in Host and Origin.
        authorities.push(address.to_string().trim_end_matches(":80").to_owned());
        authorities.push("localhost".to_owned());
    }
    authorities
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

#[derive(Debug)]
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
    auth::validate_source(request.headers(), &state.authorities, &state.browser_auth)?;
    if request.uri().path() == browser_auth::TICKET_PATH {
        let origin = request
            .headers()
            .get("origin")
            .cloned()
            .ok_or(ApiError(StatusCode::FORBIDDEN))?;
        let mut response = if request.uri().query().is_some() {
            ApiError(StatusCode::BAD_REQUEST).into_response()
        } else if request.method() == Method::OPTIONS {
            StatusCode::NO_CONTENT.into_response()
        } else if let Err(error) = auth::authenticate(request.headers(), &state.token) {
            error.into_response()
        } else {
            next.run(request).await
        };
        browser_auth::cors(response.headers_mut(), origin);
        return Ok(response);
    }
    let download = request.method() == axum::http::Method::GET
        && request.uri().path() == "/api/files/download";
    if !matches!(request.uri().path(), "/healthz" | "/readyz") && !download {
        if request.uri().path() == "/v1/ws" && !request.headers().contains_key("authorization") {
            state.browser_auth.consume(request.headers())?;
        } else {
            auth::authenticate(request.headers(), &state.token)?;
        }
    }
    // Credentials and client state must never be accepted in a URL.
    if request.uri().query().is_some() && !download {
        return Err(ApiError(StatusCode::BAD_REQUEST));
    }
    Ok(next.run(request).await)
}

async fn browser_ticket(
    State(state): State<Arc<Shared>>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    if state.cancellation.is_cancelled() {
        return Err(ApiError(StatusCode::SERVICE_UNAVAILABLE));
    }
    let origin = auth::single_header(&headers, "origin")?.ok_or(ApiError(StatusCode::FORBIDDEN))?;
    let ticket = state.browser_auth.issue(origin)?;
    Ok(Json(serde_json::json!({"ticket": ticket})).into_response())
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
    headers: HeaderMap,
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
    let protocol = headers
        .get("sec-websocket-protocol")
        .and_then(|value| value.to_str().ok())
        .filter(|value| value.starts_with(browser_auth::TICKET_PROTOCOL));
    Ok(ws
        .protocols(protocol.map(str::to_owned))
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

fn shared_service<S>(service: S) -> Arc<Mutex<S>> {
    Arc::new(Mutex::new(service))
}
