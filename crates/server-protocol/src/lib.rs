//! Versioned, transport-independent messages for the independent server.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Maximum incoming JSON message size, including fragmented messages.
pub const MAX_MESSAGE_BYTES: usize = 1024 * 1024;
/// Maximum queued outgoing messages per connection.
pub const MAX_QUEUE_MESSAGES: usize = 256;
/// Maximum queued outgoing bytes per connection, including the active write.
pub const MAX_QUEUE_BYTES: usize = 4 * 1024 * 1024;
/// Maximum simultaneous upgraded connections.
pub const MAX_CONNECTIONS: usize = 64;
/// Capabilities implemented in the first server milestone.
pub const CAPABILITIES: &[&str] = &["server.info", "connection.ping", "server.status.subscribe"];

/// Supported protocol version.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Version {
    /// Incompatible protocol generation.
    pub major: u16,
    /// Backward-compatible revision.
    pub minor: u16,
}

/// Current public protocol version.
pub const VERSION: Version = Version { major: 1, minor: 0 };

/// Client's acceptable minor-version interval for one major version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VersionOffer {
    /// Required major version.
    pub major: u16,
    /// Lowest acceptable minor version.
    pub min_minor: u16,
    /// Highest acceptable minor version.
    pub max_minor: u16,
}

/// First application message on every physical connection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hello {
    /// Version interval to negotiate.
    pub protocol: VersionOffer,
    /// Diagnostic label only; never an authenticated identity.
    pub client_id: String,
    /// Optional capabilities the client understands.
    #[serde(default)]
    pub capabilities: Vec<String>,
    /// Capabilities without which the client cannot operate.
    #[serde(default)]
    pub required_capabilities: Vec<String>,
}

impl Hello {
    /// Validate the offer and return negotiated capabilities.
    ///
    /// # Errors
    /// Rejects malformed identifiers, incompatible versions, or missing required capabilities.
    pub fn negotiate(&self) -> Result<Vec<String>, ErrorCode> {
        if !valid_id(&self.client_id)
            || self.capabilities.len() > 64
            || self.required_capabilities.len() > 64
            || self
                .capabilities
                .iter()
                .chain(&self.required_capabilities)
                .any(|s| !valid_id(s))
        {
            return Err(ErrorCode::InvalidMessage);
        }
        if self.protocol.major != VERSION.major
            || self.protocol.min_minor > self.protocol.max_minor
            || self.protocol.min_minor > VERSION.minor
        {
            return Err(ErrorCode::IncompatibleVersion);
        }
        if self
            .required_capabilities
            .iter()
            .any(|s| !CAPABILITIES.contains(&s.as_str()))
        {
            return Err(ErrorCode::UnsupportedCapability);
        }
        Ok(CAPABILITIES
            .iter()
            .filter(|cap| {
                self.capabilities
                    .iter()
                    .chain(&self.required_capabilities)
                    .any(|s| s == **cap)
            })
            .map(|cap| (*cap).to_owned())
            .collect())
    }
}

/// Validate a bounded, nonempty correlation or diagnostic identifier.
#[must_use]
pub fn valid_id(value: &str) -> bool {
    !value.is_empty() && value.len() <= 128 && !value.chars().any(char::is_control)
}

/// Messages accepted from an authenticated client.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMessage {
    /// Negotiate a new connection before sending requests.
    Hello(Hello),
    /// One connection-local RPC; this is not a durable idempotency key.
    Request {
        /// Correlation identifier, at most 128 bytes.
        request_id: String,
        /// Method name.
        method: String,
        /// Method-specific parameters.
        #[serde(default)]
        params: Value,
    },
}

/// Safe, stable machine-readable error codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// Invalid JSON, envelope, parameter, or message order.
    InvalidMessage,
    /// The offered protocol range cannot be served.
    IncompatibleVersion,
    /// A required or unnegotiated capability cannot be used.
    UnsupportedCapability,
    /// No such method is implemented.
    MethodNotFound,
    /// A subscription does not belong to this physical connection.
    SubscriptionNotFound,
    /// A bounded resource is exhausted.
    ResourceExhausted,
}

/// Admission state of the server process.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Lifecycle {
    /// New connections can be admitted.
    Ready,
    /// Shutdown has begun; new work is rejected.
    Draining,
}

/// Enforced public transport budgets.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Limits {
    /// Maximum incoming JSON message bytes.
    pub message_bytes: usize,
    /// Maximum queued outgoing messages.
    pub queue_messages: usize,
    /// Maximum queued outgoing bytes.
    pub queue_bytes: usize,
    /// Maximum simultaneous connections.
    pub connections: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            message_bytes: MAX_MESSAGE_BYTES,
            queue_messages: MAX_QUEUE_MESSAGES,
            queue_bytes: MAX_QUEUE_BYTES,
            connections: MAX_CONNECTIONS,
        }
    }
}

/// Non-secret server identity and implemented capabilities.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerInfo {
    /// Stable UUID persisted in this server's data directory.
    pub server_id: String,
    /// UUID generated for this process start.
    pub instance_id: String,
    /// Actual bound socket address.
    pub listen: String,
    /// Current admission state.
    pub lifecycle: Lifecycle,
    /// Public wire version.
    pub protocol: Version,
    /// Implemented features; business capabilities are absent in M0.
    pub capabilities: Vec<String>,
    /// Enforced transport budgets.
    pub limits: Limits,
}

/// Messages sent by the server.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    /// Successful hello, with a fresh physical connection ID.
    ServerInfo {
        /// Server identity, lifecycle, and supported features.
        info: ServerInfo,
        /// Physical connection identity, independent of `client_id`.
        connection_id: String,
        /// Capabilities negotiated on this connection.
        negotiated_capabilities: Vec<String>,
    },
    /// Successful RPC response.
    Response {
        /// Echoed correlation identifier.
        request_id: String,
        /// Method-specific response value.
        result: Value,
    },
    /// Safe protocol error without credentials or internal diagnostics.
    Error {
        /// Correlation identifier, absent before a valid request.
        request_id: Option<String>,
        /// Stable error code.
        code: ErrorCode,
    },
    /// Ephemeral connection-owned server lifecycle notification; not a durable event stream.
    Status {
        /// Subscription ID created on this physical connection.
        subscription_id: String,
        /// Current admission state.
        lifecycle: Lifecycle,
    },
}

#[cfg(test)]
mod tests;
