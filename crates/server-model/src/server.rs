//! Server identity, lifecycle and connection metadata wire messages.

use serde::{Deserialize, Serialize};

/// Maximum incoming JSON message size, including fragmented messages.
pub const MAX_MESSAGE_BYTES: usize = 1024 * 1024;
/// Maximum queued outgoing messages per connection.
pub const MAX_QUEUE_MESSAGES: usize = 256;
/// Maximum queued outgoing bytes per connection, including the active write.
pub const MAX_QUEUE_BYTES: usize = 4 * 1024 * 1024;
/// Maximum simultaneous upgraded connections.
pub const MAX_CONNECTIONS: usize = 64;

/// Baseline connection methods available before optional business services are installed.
pub const CAPABILITIES: &[&str] = &[
    "server.info",
    "connection.ping",
    "server.status.subscribe",
    "subscription.release.request",
];

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

/// Non-secret server identity, registered methods, and implemented capabilities.
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
    /// Methods admitted for negotiation, including explicit placeholders.
    pub capabilities: Vec<String>,
    /// Methods with real implementations installed by the host.
    #[serde(default)]
    pub implemented_capabilities: Vec<String>,
    /// Enforced transport budgets.
    pub limits: Limits,
}
