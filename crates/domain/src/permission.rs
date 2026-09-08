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
