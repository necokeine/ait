//! Short-lived, origin-bound credentials for browser WebSocket upgrades.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use axum::http::{HeaderMap, HeaderValue, StatusCode, Uri};
use uuid::Uuid;

use crate::{ApiError, ConfigError, auth};

pub(super) const TICKET_PATH: &str = "/v1/auth/ws-ticket";
pub(super) const TICKET_PROTOCOL: &str = "ait.ticket.";
const TICKET_TTL: Duration = Duration::from_secs(30);
const MAX_TICKETS: usize = 256;

struct Ticket {
    origin: String,
    expires: Instant,
}

#[derive(Default)]
pub(super) struct BrowserAuth {
    origins: Vec<String>,
    tickets: Mutex<HashMap<String, Ticket>>,
}

impl std::fmt::Debug for BrowserAuth {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("BrowserAuth")
            .field("origins", &self.origins)
            .finish_non_exhaustive()
    }
}

/// Validate an explicit HTTP loopback page origin, without paths or credentials.
///
/// # Errors
/// Rejects non-loopback hosts, non-HTTP schemes, and non-canonical origins.
pub fn validate_browser_origin(origin: &str) -> Result<(), ConfigError> {
    let uri: Uri = origin
        .parse()
        .map_err(|_| ConfigError::InvalidBrowserOrigin)?;
    let authority = uri.authority().ok_or(ConfigError::InvalidBrowserOrigin)?;
    let canonical = match authority.port_u16() {
        Some(0 | 80) => return Err(ConfigError::InvalidBrowserOrigin),
        Some(port) => format!("http://{}:{port}", authority.host()),
        None => format!("http://{}", authority.host()),
    };
    if uri.scheme_str() != Some("http")
        || !matches!(authority.host(), "localhost" | "127.0.0.1" | "[::1]")
        || origin != canonical
    {
        return Err(ConfigError::InvalidBrowserOrigin);
    }
    Ok(())
}

impl BrowserAuth {
    pub(super) fn new(origins: Vec<String>) -> Result<Self, ConfigError> {
        for origin in &origins {
            validate_browser_origin(origin)?;
        }
        Ok(Self {
            origins,
            tickets: Mutex::default(),
        })
    }

    pub(super) fn permits(&self, origin: &str) -> bool {
        self.origins.iter().any(|allowed| allowed == origin)
    }

    pub(super) fn issue(&self, origin: &str) -> Result<String, ApiError> {
        let now = Instant::now();
        let mut tickets = self
            .tickets
            .lock()
            .map_err(|_| ApiError(StatusCode::INTERNAL_SERVER_ERROR))?;
        tickets.retain(|_, ticket| ticket.expires > now);
        if tickets.len() >= MAX_TICKETS {
            return Err(ApiError(StatusCode::TOO_MANY_REQUESTS));
        }
        let secret = Uuid::new_v4().simple().to_string();
        tickets.insert(
            secret.clone(),
            Ticket {
                origin: origin.to_owned(),
                expires: now + TICKET_TTL,
            },
        );
        Ok(secret)
    }

    pub(super) fn consume(&self, headers: &HeaderMap) -> Result<(), ApiError> {
        let protocol = auth::single_header(headers, "sec-websocket-protocol")?
            .and_then(|value| value.strip_prefix(TICKET_PROTOCOL))
            .ok_or(ApiError(StatusCode::UNAUTHORIZED))?;
        let origin =
            auth::single_header(headers, "origin")?.ok_or(ApiError(StatusCode::FORBIDDEN))?;
        let ticket = self
            .tickets
            .lock()
            .map_err(|_| ApiError(StatusCode::INTERNAL_SERVER_ERROR))?
            .remove(protocol)
            .ok_or(ApiError(StatusCode::UNAUTHORIZED))?;
        if ticket.expires <= Instant::now() || ticket.origin != origin {
            return Err(ApiError(StatusCode::UNAUTHORIZED));
        }
        Ok(())
    }
}

pub(super) fn cors(headers: &mut HeaderMap, origin: HeaderValue) {
    headers.insert("access-control-allow-origin", origin);
    headers.insert(
        "access-control-allow-methods",
        HeaderValue::from_static("POST"),
    );
    headers.insert(
        "access-control-allow-headers",
        HeaderValue::from_static("authorization"),
    );
    headers.insert("vary", HeaderValue::from_static("Origin"));
    headers.insert("cache-control", HeaderValue::from_static("no-store"));
}

#[cfg(test)]
mod tests;
