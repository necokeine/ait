use serde::{Deserialize, Serialize};

/// Operating-system access fixed when a Run is admitted.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SandboxAccess {
    /// The Agent may inspect the Project but cannot modify files.
    ReadOnly,
    /// The Agent may write only inside the isolated Project workspace.
    WorkspaceWrite,
    /// The Agent may access the host without a filesystem sandbox.
    FullAccess,
}

/// When a native Agent harness asks the host to approve an operation.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalMode {
    /// Let the harness request approval when it determines escalation is needed.
    OnRequest,
    /// Require approval for commands the harness classifies as untrusted.
    UntrustedOnly,
}

/// Non-secret permission policy snapshotted into a Run at admission.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub struct RunPermissionProfile {
    /// Maximum filesystem access granted to the harness for the complete Run.
    pub sandbox: SandboxAccess,
    /// Native approval policy supplied to the harness for the complete Run.
    pub approval: ApprovalMode,
}

impl Default for RunPermissionProfile {
    fn default() -> Self {
        Self {
            sandbox: SandboxAccess::ReadOnly,
            approval: ApprovalMode::OnRequest,
        }
    }
}

/// Scope of one explicit native approval grant.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalGrantScope {
    /// Authorize only the protocol request being answered.
    OneShot,
    /// Authorize the explicit permission set for the current Codex turn.
    Turn,
    /// Authorize matching requests in the current app-server session.
    Session,
}

/// Kind of native Codex approval request. These are audit records, not `ToolUse`.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NativeApprovalKind {
    /// A command execution request.
    CommandExecution,
    /// A file-change request.
    FileChange,
    /// A request for an explicit filesystem/network permission profile.
    Permissions,
    /// A legacy command approval request.
    LegacyCommand,
    /// A legacy patch approval request.
    LegacyPatch,
}

/// Bounded, non-secret object a member is being asked to authorize.
///
/// Provider arguments remain adapter-owned. This projection contains only the
/// fields required to make a durable approval understandable after reconnect.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum NativeApprovalTarget {
    /// One concrete shell command and its working directory.
    Command {
        /// Redacted command text or argument projection.
        command: String,
        /// Project-relative execution context expressed as an absolute host path.
        cwd: String,
    },
    /// One managed-network destination. Network prompts are never rendered as commands.
    Network {
        /// Destination host requested by the managed network proxy.
        host: String,
        /// Network protocol requested for the destination.
        protocol: NativeNetworkProtocol,
    },
    /// Proposed file targets, without persisting patch contents.
    FileChange {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        /// Optional directory scope that Codex asks to retain for later writes.
        grant_root: Option<String>,
        /// Proposed paths and operation kinds, excluding patch contents.
        changes: Vec<NativeApprovalFileChange>,
    },
    /// The working directory associated with an explicit permission-profile request.
    Permissions {
        /// Project working directory to which the permission profile applies.
        cwd: String,
    },
}

/// Protocol-supported managed-network scheme shown in a native approval card.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum NativeNetworkProtocol {
    /// Plain HTTP.
    Http,
    /// HTTP over TLS.
    Https,
    /// SOCKS5 TCP transport.
    Socks5Tcp,
    /// SOCKS5 UDP transport.
    Socks5Udp,
}

/// One proposed file path and operation, excluding patch content.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeApprovalFileChange {
    /// Proposed file path.
    pub path: String,
    /// Proposed operation on the path.
    pub kind: NativeApprovalFileChangeKind,
}

/// File operation kinds exposed by current Codex file-change items.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NativeApprovalFileChangeKind {
    /// Add a new path.
    Add,
    /// Delete an existing path.
    Delete,
    /// Update an existing path.
    Update,
}

/// Durable lifecycle of one native approval request.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NativeApprovalStatus {
    /// The app-server is waiting for an answer.
    Pending,
    /// The member explicitly authorized the recorded scope.
    Approved,
    /// The member refused the operation while allowing the turn to continue.
    Denied,
    /// The member refused the operation and requested turn cancellation.
    Cancelled,
    /// The app-server withdrew the request or its turn ended before an answer.
    Expired,
}
