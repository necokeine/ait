//! Workspace setup and script RPC payloads copied from Paseo's public schemas.

use serde::{Deserialize, Serialize};

/// Canonical setup and script methods implemented by the independent server.
pub const CAPABILITIES: &[&str] = &[
    "workspace.setup.status.request",
    "workspace.setup.run.request",
    "workspace.script.list.request",
    "workspace.script.start.request",
    "workspace.script.stop.request",
];

/// Select one workspace.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceSetupRequest {
    /// Durable workspace identity.
    pub workspace_id: String,
}

/// Select one configured workspace script.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceScriptRequest {
    /// Durable workspace identity.
    pub workspace_id: String,
    /// Exact key under `scripts` in `paseo.json`.
    pub script_name: String,
}

/// State of one setup command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceSetupCommandStatus {
    /// The command has started but has not exited.
    Running,
    /// The command exited successfully.
    Completed,
    /// The command exited unsuccessfully.
    Failed,
}

/// Snapshot of one command in a workspace setup run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceSetupCommand {
    /// One-based command position.
    pub index: usize,
    /// Shell command from `paseo.json`.
    pub command: String,
    /// Directory in which the command runs.
    pub cwd: String,
    /// Bounded combined output.
    pub log: String,
    /// Current command state.
    pub status: WorkspaceSetupCommandStatus,
    /// Process exit code, or null while running or when terminated by a signal.
    pub exit_code: Option<i32>,
    /// Elapsed milliseconds after completion.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
}

/// Paseo worktree setup detail payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceSetupDetail {
    /// Fixed Paseo detail discriminator.
    #[serde(rename = "type")]
    pub kind: String,
    /// Backing worktree or directory path.
    pub worktree_path: String,
    /// Git branch when known.
    pub branch_name: String,
    /// Rendered bounded setup transcript.
    pub log: String,
    /// Per-command snapshots.
    pub commands: Vec<WorkspaceSetupCommand>,
    /// Present only when output was truncated.
    #[serde(skip_serializing_if = "is_false")]
    pub truncated: bool,
}

/// Overall setup lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceSetupStatus {
    /// Setup commands are running.
    Running,
    /// Every setup command completed.
    Completed,
    /// A setup command failed.
    Failed,
    /// Automation awaits explicit trust.
    Blocked,
}

/// Persisted untrusted checkout provenance exposed with a blocked snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum WorkspaceBlockedSource {
    /// A change request from another repository.
    ChangeRequest {
        /// Forge identifier.
        forge: String,
        /// Positive change-request number.
        number: u64,
        /// Repository that supplied the head branch.
        head_repository: String,
    },
}

/// Cached setup status returned by polling and progress events.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceSetupSnapshot {
    /// Overall setup lifecycle.
    pub status: WorkspaceSetupStatus,
    /// Worktree setup transcript.
    pub detail: WorkspaceSetupDetail,
    /// Safe failure text.
    pub error: Option<String>,
    /// Present while automation is blocked for untrusted code.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blocked_source: Option<WorkspaceBlockedSource>,
}

/// Setup status polling result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceSetupStatusResult {
    /// Requested workspace identity.
    pub workspace_id: String,
    /// Last in-memory snapshot or a derived blocked snapshot.
    pub snapshot: Option<WorkspaceSetupSnapshot>,
}

/// Explicit setup approval/start result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceSetupRunResult {
    /// Requested workspace identity.
    pub workspace_id: String,
    /// Whether an automation block was cleared and a setup run started.
    pub started: bool,
    /// Safe failure text.
    pub error: Option<String>,
}

/// Script classification from `paseo.json`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceScriptType {
    /// One-shot shell command.
    Script,
    /// Long-running service with an optional TCP port.
    Service,
}

/// Script process lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceScriptLifecycle {
    /// The child process has not exited.
    Running,
    /// The child process is absent or has exited.
    Stopped,
}

/// Service health projection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceScriptHealth {
    /// Health probing succeeded.
    Healthy,
    /// Health probing failed.
    Unhealthy,
}

/// Public script state copied from Paseo's `WorkspaceScriptPayloadSchema`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceScript {
    /// Exact configuration key.
    pub script_name: String,
    /// Plain script or service.
    #[serde(rename = "type")]
    pub kind: WorkspaceScriptType,
    /// Stable service hostname; plain scripts use their script name.
    pub hostname: String,
    /// Configured or allocated service port.
    pub port: Option<u16>,
    /// Loopback proxy URL when a proxy is installed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub local_proxy_url: Option<String>,
    /// Public proxy URL when configured.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub public_proxy_url: Option<String>,
    /// Backward-compatible preferred proxy URL.
    pub proxy_url: Option<String>,
    /// Current process lifecycle.
    pub lifecycle: WorkspaceScriptLifecycle,
    /// Service health, or null when unavailable/not applicable.
    pub health: Option<WorkspaceScriptHealth>,
    /// Last exit code.
    pub exit_code: Option<i32>,
    /// Logical terminal/process identity.
    pub terminal_id: Option<String>,
}

/// Script list result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceScriptListResult {
    /// Requested workspace identity.
    pub workspace_id: String,
    /// Configured scripts plus running orphan entries.
    pub scripts: Vec<WorkspaceScript>,
    /// Safe failure text.
    pub error: Option<String>,
}

/// Script start/stop result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceScriptMutationResult {
    /// Requested workspace identity.
    pub workspace_id: String,
    /// Requested script key.
    pub script_name: String,
    /// Updated script state.
    pub script: Option<WorkspaceScript>,
    /// Safe failure text.
    pub error: Option<String>,
}

#[allow(clippy::trivially_copy_pass_by_ref)]
const fn is_false(value: &bool) -> bool {
    !*value
}

#[cfg(test)]
mod tests;
