//! Authenticated local HTTP and WebSocket transport for the independent server.

mod auth;
mod browser_auth;
mod capabilities;
mod connection;
mod files;
mod listener;
mod outbound;
mod terminal_activity;
mod workspace_cleanup;

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use model::Runtime;
use std::time::Duration;

use axum::extract::{ConnectInfo, Request, State, WebSocketUpgrade};
use axum::http::{HeaderMap, Method, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use filesystem::service::checkout::Checkout;
use filesystem::service::files::Files;
use filesystem::service::forge::Forge;
use filesystem::service::github_projects::GithubProjects;
use filesystem::service::workspace_recovery::WorkspaceRecovery;
use filesystem::service::worktrees::{WorkspaceWorktrees, Worktrees};
use metadata::service::daemon::Daemon;
use metadata::service::directory::Directory;
use metadata::service::workspace_automation::WorkspaceAutomation;
use metadata::service::workspace_labels::WorkspaceLabels;
use metadata::service::workspace_state::WorkspaceState;
use protocol::{Lifecycle, Limits, ServerInfo, VERSION};
use provider::service::agent_execution::AgentExecution;
use provider::service::agent_runtime::AgentRuntimeDirectory;
use provider::service::agents::Agents;
use secrecy::SecretString;
use tokio::sync::Semaphore;
use tower_http::timeout::TimeoutLayer;
use tower_http::trace::TraceLayer;

pub use auth::validate_token;
pub use browser_auth::validate_browser_origin;
use capabilities::installed_capabilities;
pub use listener::LocalAddress;
use listener::allowed_authorities;

/// Invalid startup configuration for the transport.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// Only addresses with an assigned port are supported.
    #[error("server requires an address with an assigned port")]
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
    /// A shared service failed before transport initialization completed.
    #[error("shared service unavailable during API initialization")]
    ServiceInitialization,
}

#[derive(Debug)]
struct Shared {
    relay: relay::Connector,
    runtime: Arc<Runtime>,
    token: SecretString,
    browser_auth: browser_auth::BrowserAuth,
    authorities: Vec<String>,
    wildcard_listener: bool,
    connections: Arc<Semaphore>,
    metadata: Arc<metadata::dispatch::State>,
    filesystem: Arc<filesystem::dispatch::State>,
    provider: Arc<provider::dispatch::State>,
    terminal: Arc<terminal::dispatch::State>,
    voice: Arc<voice::dispatch::State>,
    schedule: schedule::dispatch::State,
    browser: browser::dispatch::State,
    workspace_names: Option<metadata::service::workspace_names::WorkspaceNames>,
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
    /// Bounded model-backed wording generation shared by title and Git use cases.
    pub metadata_generator: Option<Arc<dyn metadata::ports::generation::MetadataGenerator>>,
    /// First-prompt workspace naming with independently drained background work.
    pub workspace_names: Option<metadata::service::workspace_names::WorkspaceNames>,
    /// Persistent timed Agent executions.
    pub schedules: Option<schedule::service::Schedules>,
    /// Connection-owned browser automation broker.
    pub browser: Option<browser::broker::Broker>,
    /// Orchestration skill selection and installation.
    pub skills: Option<filesystem::service::skills::Skills>,
    /// Durable push registration and lease renewal.
    pub push_tokens: Option<metadata::service::push::PushTokens>,
    /// Connection-owned voice and dictation with independently selected speech engines.
    pub speech: Option<voice::service::Speech>,
    /// Local PTY terminal lifecycle, input, capture, and streaming.
    pub terminals: Option<terminal::service::Terminals>,
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
    /// Background origin fetches for actively observed workspace repositories.
    pub git_fetch: Option<filesystem::service::git_fetch::GitFetch>,
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
    pub workspace_automation: Option<Arc<Mutex<WorkspaceAutomation>>>,
    /// Workspace attention and archived-placement recovery use cases.
    pub workspace_state: Option<WorkspaceState>,
    /// Paseo-owned Git worktree lifecycle use cases.
    pub worktrees: Option<Arc<Mutex<Worktrees>>>,
}

pub use metadata::rpc::daemon::LifecycleIntent;

impl Shared {
    fn start_draining(&self) {
        self.relay.begin_shutdown();
        if let Some(schedules) = &self.schedule.schedules {
            schedules.stop();
        }
        if let Some(generator) = &self.filesystem.metadata_generator {
            generator.shutdown();
        }
        if let Some(names) = &self.workspace_names {
            names.shutdown();
        }
        self.metadata.start_draining();
    }
}

/// Cloneable transport owner; the process host owns the listener and shutdown deadline.
#[derive(Debug, Clone)]
pub struct Api {
    shared: Arc<Shared>,
}

fn runtime_info(
    address: SocketAddr,
    server_id: String,
    instance_id: String,
    services: &Services,
) -> Arc<Runtime> {
    let implemented_capabilities = installed_capabilities(services);
    let capabilities = registered_capabilities(&implemented_capabilities);
    Arc::new(Runtime::new(ServerInfo {
        server_id,
        version: Some(env!("CARGO_PKG_VERSION").to_owned()),
        instance_id,
        listen: address.to_string(),
        lifecycle: Lifecycle::Ready,
        protocol: VERSION,
        capabilities,
        features: capabilities::features(services),
        implemented_capabilities,
        limits: Limits::default(),
    }))
}

impl Api {
    /// Construct transport state for an already-bound TCP listener.
    ///
    /// `server_id` is persisted by the host; `instance_id` is unique to this process start.
    /// `token` authenticates the info and upgrade endpoints and is never returned to clients.
    /// `services` selects implemented methods; catalog placeholders remain negotiable.
    ///
    /// # Errors
    /// Returns an error for unassigned ports or invalid tokens.
    pub fn new(
        address: SocketAddr,
        server_id: String,
        instance_id: String,
        token: SecretString,
        services: Services,
    ) -> Result<Self, ConfigError> {
        use secrecy::ExposeSecret;
        if address.port() == 0 {
            return Err(ConfigError::InvalidAddress);
        }
        validate_token(token.expose_secret())?;
        let session_events = services
            .agent_execution
            .as_ref()
            .map(AgentExecution::events)
            .unwrap_or_default();
        compose_automation_events(services.workspace_automation.as_ref(), &session_events)?;
        let creations = creation_receipts(&services);
        let relay = relay::Connector::new(
            address,
            token.clone(),
            server_id.clone(),
            instance_id.clone(),
        );
        let runtime = runtime_info(address, server_id, instance_id, &services);
        let worktrees = services.worktrees;
        let (directory, has_git_fetch) = compose_directory(
            services.directory,
            worktrees.as_ref(),
            services.git_fetch,
            &runtime,
            &session_events,
            services.workspace_automation.is_some(),
        );
        let directory_changes = directory.as_ref().and_then(Directory::changes);
        let metadata = Arc::new(metadata::dispatch::State {
            push_tokens: services.push_tokens.map(shared_service),
            runtime: runtime.clone(),
            daemon: services.daemon.map(shared_service),
            directory: directory.map(shared_service),
            workspace_labels: services.workspace_labels.map(shared_service),
            workspace_automation: services.workspace_automation,
            workspace_state: services.workspace_state.map(shared_service),
            session_events,
            creations,
            has_agent_execution: services.agent_execution.is_some(),
            has_terminals: services.terminals.is_some(),
            has_git_fetch,
        });
        let filesystem = Arc::new(filesystem::dispatch::State {
            metadata_generator: services.metadata_generator,
            runtime: runtime.clone(),
            checkout: services.checkout.map(shared_service),
            forge: services.forge.map(shared_service),
            files: services.files.map(shared_service),
            github_projects: services.github_projects.map(shared_service),
            worktrees,
            workspace_recovery: services.workspace_recovery.map(shared_service),
            skills: services.skills.map(shared_service),
            workspace_automation: metadata.workspace_automation.clone(),
        });
        let provider = Arc::new(provider::dispatch::State {
            runtime: runtime.clone(),
            agents: services.agents.map(shared_service),
            agent_runtime: services.agent_runtime.map(shared_service),
            agent_execution: services.agent_execution,
            directory_changes,
            has_terminals: services.terminals.is_some(),
        });
        let terminal = terminal_activity::compose(
            &runtime,
            services.terminals,
            address,
            &metadata.session_events,
        );
        let api = Self {
            shared: Arc::new(Shared {
                relay,
                schedule: schedule::dispatch::State {
                    schedules: services.schedules,
                },
                browser: browser::dispatch::State {
                    broker: services.browser,
                },
                voice: Arc::new(voice::dispatch::State {
                    runtime: runtime.clone(),
                    speech: services.speech,
                }),
                workspace_names: services.workspace_names,
                runtime,
                metadata,
                filesystem,
                provider,
                terminal,
                token,
                browser_auth: browser_auth::BrowserAuth::default(),
                authorities: allowed_authorities(address),
                wildcard_listener: address.ip().is_unspecified(),
                connections: Arc::new(Semaphore::new(protocol::MAX_CONNECTIONS)),
            }),
        };
        workspace_cleanup::configure(&api.shared)?;
        terminal::connection::maintain(&api.shared.terminal);
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
    pub fn browser(&self) -> Option<browser::broker::Broker> {
        self.shared.browser.broker.clone()
    }

    /// Build routes with origin checks, bounded HTTP handling, and metadata-only tracing.
    pub fn router(&self) -> Router {
        Router::new()
            .route("/healthz", get(health))
            .route("/readyz", get(ready))
            .route("/v1/server/info", get(info))
            .route("/v1/ws", get(upgrade))
            .route(
                "/api/relay/control",
                get(relay_status).put(relay_start).delete(relay_stop),
            )
            .route(browser_auth::TICKET_PATH, post(browser_ticket))
            .route("/api/files/download", get(files::download))
            .route(terminal_activity::PATH, post(terminal_activity::report))
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
        self.shared.relay.stop().await;
        self.shared.tasks.wait().await;
        if let Some(names) = &self.shared.workspace_names {
            names.wait_closed().await;
        }
        if let Some(schedules) = &self.shared.schedule.schedules
            && schedules.shutdown().await.is_err()
        {
            tracing::error!("schedule shutdown failed");
        }
        if let Some(terminals) = self.shared.terminal.terminals.clone() {
            let result = tokio::task::spawn_blocking(move || {
                terminals
                    .lock()
                    .map_err(|_| terminal::Error::Io)?
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

fn registered_capabilities(implemented: &[String]) -> Vec<String> {
    let mut capabilities = implemented.to_vec();
    capabilities.push(protocol::single::CAPABILITY.to_owned());
    for method in protocol::methods::PASEO_METHODS {
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
    let destination_authorities;
    let authorities = if state.wildcard_listener {
        let local = request
            .extensions()
            .get::<ConnectInfo<LocalAddress>>()
            .ok_or(ApiError(StatusCode::FORBIDDEN))?;
        destination_authorities = local.0.authorities();
        &destination_authorities
    } else {
        &state.authorities
    };
    auth::validate_source(request.headers(), authorities, &state.browser_auth)?;
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
    let terminal_activity =
        request.method() == Method::POST && request.uri().path() == terminal_activity::PATH;
    if !matches!(request.uri().path(), "/healthz" | "/readyz") && !download && !terminal_activity {
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

async fn relay_status(State(state): State<Arc<Shared>>) -> Json<relay::Status> {
    Json(state.relay.status().await)
}

async fn relay_start(
    State(state): State<Arc<Shared>>,
    Json(grant): Json<relay::ControlGrant>,
) -> Result<StatusCode, ApiError> {
    if state.cancellation.is_cancelled() {
        return Err(ApiError(StatusCode::SERVICE_UNAVAILABLE));
    }
    state
        .relay
        .start(grant)
        .await
        .map_err(|_| ApiError(StatusCode::BAD_REQUEST))?;
    Ok(StatusCode::ACCEPTED)
}

async fn relay_stop(State(state): State<Arc<Shared>>) -> StatusCode {
    state.relay.stop().await;
    StatusCode::NO_CONTENT
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
        .max_message_size(protocol::MAX_MESSAGE_BYTES)
        .max_frame_size(protocol::MAX_MESSAGE_BYTES)
        .write_buffer_size(0)
        .max_write_buffer_size(protocol::MAX_QUEUE_BYTES)
        .on_upgrade(move |socket| async move {
            let (_tracking, _permit) = (tracking, permit);
            Box::pin(connection::serve(socket, state)).await;
        }))
}

#[cfg(test)]
mod tests;

fn compose_directory(
    directory: Option<Directory>,
    worktrees: Option<&Arc<Mutex<Worktrees>>>,
    git_fetch: Option<filesystem::service::git_fetch::GitFetch>,
    runtime: &Arc<Runtime>,
    events: &metadata::service::session::SessionEvents,
    has_automation: bool,
) -> (Option<Directory>, bool) {
    let has_git_fetch = directory.is_some() && git_fetch.is_some();
    let directory = directory.map(|directory| {
        let project_events = events.clone();
        let directory = directory.with_project_updates(Arc::new(move |mutation| {
            use metadata::ports::registry::MutationKind;
            use metadata::protocol::session::SessionEventKind;

            let payload = if mutation.kind == MutationKind::Upsert {
                mutation.project.as_ref().map(|project| {
                    serde_json::json!({
                        "kind": "upsert",
                        "project": metadata::rpc::directory::project_descriptor(project),
                    })
                })
            } else {
                Some(serde_json::json!({
                    "kind": "remove",
                    "projectId": mutation.project_id,
                }))
            };
            if let Some(payload) = payload {
                project_events.publish(SessionEventKind::ProjectUpdate, &payload);
            }
        }));
        let directory = if has_automation {
            let setup_events = events.clone();
            directory.with_workspace_updates(Arc::new(move |mutation| {
                use metadata::ports::registry::MutationKind;
                use metadata::protocol::session::SessionEventKind;

                if mutation.kind != MutationKind::Upsert {
                    return;
                }
                let Some(workspace) = &mutation.workspace else {
                    return;
                };
                let Some(source) = &workspace.untrusted_source else {
                    return;
                };
                setup_events.publish(
                    SessionEventKind::WorkspaceSetupProgress,
                    &serde_json::json!({
                        "workspaceId": workspace.workspace_id,
                        "status": "blocked",
                        "detail": {
                            "type": "worktree_setup",
                            "worktreePath": workspace.worktree_root.as_ref().unwrap_or(&workspace.cwd),
                            "branchName": workspace.branch.as_deref().unwrap_or_default(),
                            "log": "",
                            "commands": [],
                        },
                        "error": null,
                        "blockedSource": source,
                    }),
                );
            }))
        } else {
            directory
        };
        let directory = if let Some(worktrees) = worktrees {
            directory.with_worktrees(Arc::new(WorkspaceWorktrees::new(worktrees.clone())))
        } else {
            directory
        };
        if let Some(fetch) = git_fetch {
            directory.with_git_observer(fetch.start(runtime, events.clone()))
        } else {
            directory
        }
    });
    (directory, has_git_fetch)
}

fn compose_automation_events(
    automation: Option<&Arc<Mutex<WorkspaceAutomation>>>,
    events: &metadata::service::session::SessionEvents,
) -> Result<(), ConfigError> {
    use metadata::ports::workspace_automation::AutomationEvent;
    use metadata::protocol::session::SessionEventKind;

    let Some(automation) = automation else {
        return Ok(());
    };
    let events = events.clone();
    automation
        .lock()
        .map_err(|_| ConfigError::ServiceInitialization)?
        .set_event_sink(Arc::new(move |event| {
            let (kind, payload) = match event {
                AutomationEvent::Scripts {
                    workspace_id,
                    scripts,
                } => (
                    SessionEventKind::ScriptStatus,
                    serde_json::json!({
                        "workspaceId": workspace_id,
                        "scripts": scripts
                            .into_iter()
                            .map(metadata::rpc::workspace_automation::script)
                            .collect::<Vec<_>>(),
                    }),
                ),
                AutomationEvent::Setup {
                    workspace_id,
                    snapshot,
                } => {
                    let mut payload = serde_json::to_value(
                        metadata::rpc::workspace_automation::setup_snapshot(snapshot, None),
                    )
                    .unwrap_or_default();
                    payload["workspaceId"] = serde_json::json!(workspace_id);
                    (SessionEventKind::WorkspaceSetupProgress, payload)
                }
            };
            events.publish(kind, &payload);
        }));
    Ok(())
}

fn shared_service<S>(service: S) -> Arc<Mutex<S>> {
    Arc::new(Mutex::new(service))
}

fn creation_receipts(services: &Services) -> metadata::service::creation::Creations {
    services
        .agent_execution
        .as_ref()
        .map(AgentExecution::creations)
        .or_else(|| services.directory.as_ref().map(Directory::creations))
        .unwrap_or_default()
}
