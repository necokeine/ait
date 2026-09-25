//! Versioned, transport-independent messages for the independent server.

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub mod methods;
pub mod subscription;

pub use server_model::server::{
    CAPABILITIES, Lifecycle, Limits, MAX_CONNECTIONS, MAX_MESSAGE_BYTES, MAX_QUEUE_BYTES,
    MAX_QUEUE_MESSAGES, ServerInfo, VERSION, Version,
};

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
        self.negotiate_available(
            &CAPABILITIES
                .iter()
                .map(|s| (*s).to_owned())
                .collect::<Vec<_>>(),
        )
    }

    /// Negotiate against capabilities actually installed by the composition root.
    ///
    /// # Errors
    /// Rejects malformed offers, incompatible versions, and missing required capabilities.
    pub fn negotiate_available(&self, available: &[String]) -> Result<Vec<String>, ErrorCode> {
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
            .any(|s| !available.contains(s))
        {
            return Err(ErrorCode::UnsupportedCapability);
        }
        Ok(available
            .iter()
            .filter(|cap| {
                self.capabilities
                    .iter()
                    .chain(&self.required_capabilities)
                    .any(|s| s == *cap)
            })
            .cloned()
            .collect())
    }
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
    /// Uncorrelated client notification using a canonical event method.
    Event {
        /// Event method name.
        method: String,
        /// Method-specific event payload.
        #[serde(default)]
        params: Value,
    },
    /// Reply to a server-initiated operation using a canonical response method.
    Response {
        /// Optional original server request identifier.
        #[serde(default)]
        request_id: Option<String>,
        /// Response method name.
        method: String,
        /// Method-specific response payload.
        #[serde(default)]
        params: Value,
    },
}

pub use server_model::{ErrorCode, ServerMessage, valid_id};

#[cfg(test)]
mod tests;
