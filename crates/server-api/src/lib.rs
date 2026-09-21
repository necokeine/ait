//! Authenticated local HTTP and WebSocket transport for the independent server.

mod agents;
mod auth;
mod connection;
mod jobs;
mod outbound;
mod projects;

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
use server_application::Projects;
use server_application::agents::Agents;
use server_protocol::{CAPABILITIES, Lifecycle, Limits, ServerInfo, VERSION};
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
    projects: Option<Arc<Mutex<Projects>>>,
    agents: Option<Arc<Mutex<Agents>>>,
    jobs: Arc<Semaphore>,
}

/// Optional independently composed business services; only installed methods are advertised.
#[derive(Debug, Default)]
pub struct Services {
    /// Project ownership and registration use cases.
    pub projects: Option<Projects>,
    /// Versioned Agent presets and explicit default selection.
    pub agents: Option<Agents>,
}

impl Shared {
    fn info(&self) -> ServerInfo {
        let mut info = self.info.clone();
        if self.cancellation.is_cancelled() {
            info.lifecycle = Lifecycle::Draining;
        }
        info
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
    /// `services` installs capabilities only for initialized independent application services.
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
        let mut capabilities: Vec<String> = CAPABILITIES.iter().map(|s| (*s).to_owned()).collect();
        if services.projects.is_some() {
            capabilities.extend(
                server_protocol::project_lease::CAPABILITIES
                    .iter()
                    .map(|s| (*s).to_owned()),
            );
        }
        if services.agents.is_some() {
            capabilities.extend(
                server_protocol::agent::CAPABILITIES
                    .iter()
                    .map(|s| (*s).to_owned()),
            );
        }
        Ok(Self {
            shared: Arc::new(Shared {
                info: ServerInfo {
                    server_id,
                    instance_id,
                    listen: address.to_string(),
                    lifecycle: Lifecycle::Ready,
                    protocol: VERSION,
                    capabilities,
                    limits: Limits::default(),
                },
                token,
                authorities,
                cancellation: CancellationToken::new(),
                tasks: TaskTracker::new(),
                connections: Arc::new(Semaphore::new(server_protocol::MAX_CONNECTIONS)),
                admission: Mutex::new(()),
                projects: services
                    .projects
                    .map(|projects| Arc::new(Mutex::new(projects))),
                agents: services.agents.map(|agents| Arc::new(Mutex::new(agents))),
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

    /// Wait for all admitted upgrades and connections after calling `begin_shutdown`.
    /// The process host must bound this wait with its shutdown deadline.
    pub async fn wait_closed(&self) {
        self.shared.tasks.wait().await;
    }

    /// Wait until shutdown has closed admission; useful for bounding the host's drain phase.
    pub async fn wait_draining(&self) {
        self.shared.cancellation.cancelled().await;
    }
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
    if !matches!(request.uri().path(), "/healthz" | "/readyz") {
        auth::authenticate(request.headers(), &state.token)?;
    }
    // Credentials and client state must never be accepted in a URL.
    if request.uri().query().is_some() {
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
