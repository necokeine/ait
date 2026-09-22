//! Versioned, transport-independent messages for the independent server.

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub mod agent;
pub mod agent_lifecycle;
pub mod checkout;
pub mod daemon;
pub mod directory;
pub mod forge;
pub mod methods;
pub mod project;
pub mod project_config;
pub mod project_icon;
pub mod project_lease;
pub mod subscription;
pub mod workspace;
pub mod workspace_automation;
pub mod workspace_labels;
pub mod workspace_state;
pub mod worktrees;

/// Maximum incoming JSON message size, including fragmented messages.
pub const MAX_MESSAGE_BYTES: usize = 1024 * 1024;
/// Maximum queued outgoing messages per connection.
pub const MAX_QUEUE_MESSAGES: usize = 256;
/// Maximum queued outgoing bytes per connection, including the active write.
pub const MAX_QUEUE_BYTES: usize = 4 * 1024 * 1024;
/// Maximum simultaneous upgraded connections.
pub const MAX_CONNECTIONS: usize = 64;
/// Capabilities implemented in the first server milestone.
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
    /// New work is no longer accepted.
    ServerDraining,
    /// Only independent Git roots with an existing HEAD are supported.
    UnsupportedWorkspace,
    /// The path belongs to an old managed project.
    LegacyProject,
    /// Another process holds a path or project identity lease.
    ProjectBusy,
    /// This is not a supported independent database schema.
    UnsupportedFormat,
    /// A durable key was reused for different parameters.
    IdempotencyConflict,
    /// Project identity or path conflicts with the catalog.
    IdentityConflict,
    /// No such catalog project exists.
    ProjectNotFound,
    /// This process does not hold the project open.
    ProjectNotOpen,
    /// An ownership generation has expired.
    StaleOwner,
    /// Project storage, filesystem, or Git failed.
    ProjectIo,
    /// No Agent has this ID.
    AgentNotFound,
    /// No immutable Agent revision has this number.
    AgentRevisionNotFound,
    /// The current Agent revision differs from the expected revision.
    AgentRevisionConflict,
    /// The explicit default changed since the caller read it.
    AgentDefaultConflict,
    /// A disabled Agent cannot be selected.
    AgentDisabled,
    /// A default must be deselected before it is disabled.
    AgentIsDefault,
    /// A transient catalog transaction lock is held.
    CatalogBusy,
    /// Agent storage failed.
    AgentIo,
    /// A workspace record does not exist.
    WorkspaceNotFound,
    /// Project or workspace registry storage failed.
    RegistryIo,
    /// Persisted daemon configuration is invalid.
    DaemonConfigInvalid,
    /// Daemon configuration or runtime I/O failed.
    DaemonIo,
    /// Normalized workspace label name is empty.
    LabelNameEmpty,
    /// No workspace label has the requested name.
    LabelNotFound,
    /// Another workspace label already owns the requested name.
    LabelNameTaken,
    /// A compound label/catalog write has an uncertain durable outcome.
    WorkspaceLabelStorageUncertain,
}

impl ErrorCode {
    /// Safe explanation without paths, database diagnostics, or credentials.
    #[must_use]
    pub fn message(self) -> &'static str {
        match self {
            Self::InvalidMessage => "Invalid message or parameters",
            Self::IncompatibleVersion => "Incompatible protocol version",
            Self::UnsupportedCapability => "Capability was not negotiated",
            Self::MethodNotFound => "Unknown method",
            Self::SubscriptionNotFound => "Unknown connection subscription",
            Self::ResourceExhausted => "Resource budget exhausted",
            Self::ServerDraining => "Server is draining",
            Self::UnsupportedWorkspace => "An independent Git root with HEAD is required",
            Self::LegacyProject => "Legacy managed projects are unsupported",
            Self::ProjectBusy => "Project is owned by another process",
            Self::UnsupportedFormat => "Unsupported independent database format",
            Self::IdempotencyConflict => "Key was already used with different parameters",
            Self::IdentityConflict => "Project identity conflicts with its registration",
            Self::ProjectNotFound => "Project is not registered",
            Self::ProjectNotOpen => "Project is not open in this server",
            Self::StaleOwner => "Project owner has changed",
            Self::ProjectIo => "Project I/O failed; retry with the same key",
            Self::AgentNotFound => "Agent was not found",
            Self::AgentRevisionNotFound => "Agent revision does not exist",
            Self::AgentRevisionConflict => "Agent revision has changed",
            Self::AgentDefaultConflict => "Default Agent selection has changed",
            Self::AgentDisabled => "Agent is disabled",
            Self::AgentIsDefault => "Deselect the default Agent before disabling it",
            Self::CatalogBusy => "Catalog is busy",
            Self::AgentIo => "Agent I/O failed; retry with the same key",
            Self::WorkspaceNotFound => "Workspace is not registered",
            Self::RegistryIo => "Project or workspace registry I/O failed",
            Self::DaemonConfigInvalid => "Daemon configuration is invalid",
            Self::DaemonIo => "Daemon configuration or runtime I/O failed",
            Self::LabelNameEmpty => "Workspace label name cannot be empty",
            Self::LabelNotFound => "Workspace label was not found",
            Self::LabelNameTaken => "A workspace label with that name already exists",
            Self::WorkspaceLabelStorageUncertain => {
                "Workspace label storage outcome is uncertain; restart before retrying"
            }
        }
    }

    /// Whether the unchanged request may succeed after a transient condition clears.
    #[must_use]
    pub fn retryable(self) -> bool {
        matches!(
            self,
            Self::ResourceExhausted
                | Self::ServerDraining
                | Self::ProjectBusy
                | Self::ProjectIo
                | Self::CatalogBusy
                | Self::AgentIo
                | Self::RegistryIo
                | Self::DaemonIo
                | Self::WorkspaceLabelStorageUncertain
        )
    }
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
    /// Features actually installed by the host.
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
        /// Safe explanation derived solely from the code.
        #[serde(default)]
        message: String,
        /// Retry transient errors with the original durable key.
        #[serde(default)]
        retryable: bool,
    },
    /// Ephemeral connection-owned server lifecycle notification; not a durable event stream.
    Status {
        /// Subscription ID created on this physical connection.
        subscription_id: String,
        /// Current admission state.
        lifecycle: Lifecycle,
    },
    /// Ephemeral method-tagged event owned by a connection subscription.
    Event {
        /// Canonical event method.
        method: String,
        /// Method-specific payload.
        params: Value,
    },
}

#[cfg(test)]
mod tests;
