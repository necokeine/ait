//! Metadata-owned server methods and ping payload.

use serde::{Deserialize, Serialize};

pub use server_model::server::{
    Lifecycle, Limits, MAX_CONNECTIONS, MAX_MESSAGE_BYTES, MAX_QUEUE_BYTES, MAX_QUEUE_MESSAGES,
    ServerInfo, VERSION, Version,
};

/// Capabilities implemented in the first server milestone.
pub const CAPABILITIES: &[&str] = &[
    "server.info",
    "connection.ping",
    "server.status.subscribe",
    "subscription.release.request",
];

/// Application ping parameters; extra fields remain accepted for compatibility.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ping {
    /// Bounded correlation nonce echoed by the server.
    pub nonce: String,
}

/// Canonical client activity heartbeat event owned by metadata.
pub const HEARTBEAT_METHOD: &str = "session.heartbeat";
