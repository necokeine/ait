//! Shared response envelopes and stable error codes.

use crate::server::{Lifecycle, ServerInfo};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Safe, stable machine-readable error codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// Terminal native process or screen I/O failed.
    TerminalIo,
    /// Terminal no longer exists.
    TerminalNotFound,
    /// Invalid JSON, envelope, parameter, or message order.
    InvalidMessage,
    /// The offered protocol range cannot be served.
    IncompatibleVersion,
    /// A required or unnegotiated capability cannot be used.
    UnsupportedCapability,
    /// No such canonical method is registered.
    MethodNotFound,
    /// The method is registered, but its behavior is awaiting implementation.
    NotImplemented,
    /// A subscription does not belong to this physical connection.
    SubscriptionNotFound,
    /// A bounded resource is exhausted.
    ResourceExhausted,
    /// New work is no longer accepted.
    ServerDraining,
    /// This is not a supported independent database schema.
    UnsupportedFormat,
    /// A durable key was reused for different parameters.
    IdempotencyConflict,
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
            Self::TerminalIo => "Terminal I/O failed",
            Self::TerminalNotFound => "Terminal not found",
            Self::InvalidMessage => "Invalid message or parameters",
            Self::IncompatibleVersion => "Incompatible protocol version",
            Self::UnsupportedCapability => "Capability was not negotiated",
            Self::MethodNotFound => "Unknown method",
            Self::NotImplemented => "Method is not implemented yet",
            Self::SubscriptionNotFound => "Unknown connection subscription",
            Self::ResourceExhausted => "Resource budget exhausted",
            Self::ServerDraining => "Server is draining",
            Self::UnsupportedFormat => "Unsupported independent database format",
            Self::IdempotencyConflict => "Key was already used with different parameters",
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
                | Self::ProjectIo
                | Self::CatalogBusy
                | Self::AgentIo
                | Self::RegistryIo
                | Self::DaemonIo
                | Self::WorkspaceLabelStorageUncertain
        )
    }
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
