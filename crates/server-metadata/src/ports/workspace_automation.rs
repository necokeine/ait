//! Blocking process boundary for workspace setup and `paseo.json` scripts.

use std::fmt::Debug;

/// Configured script type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScriptType {
    /// One-shot shell command.
    Script,
    /// Long-running service.
    Service,
}

/// One configured script and its current runtime projection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptSnapshot {
    /// Exact `paseo.json` key.
    pub name: String,
    /// Plain script or service.
    pub kind: ScriptType,
    /// Stable hostname projection.
    pub hostname: String,
    /// Configured service port.
    pub port: Option<u16>,
    /// Whether the child is still running.
    pub running: bool,
    /// Last process exit code.
    pub exit_code: Option<i32>,
    /// Logical child identity.
    pub terminal_id: Option<String>,
}

/// One setup command snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetupCommandSnapshot {
    /// One-based command position.
    pub index: usize,
    /// Shell command.
    pub command: String,
    /// Execution directory.
    pub cwd: String,
    /// Bounded combined output.
    pub log: String,
    /// Whether the command is still running.
    pub running: bool,
    /// Exit code after completion.
    pub exit_code: Option<i32>,
    /// Elapsed milliseconds after completion.
    pub duration_ms: Option<u64>,
}

/// Setup lifecycle independent of the wire schema.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetupLifecycle {
    /// Commands are running.
    Running,
    /// Every command completed.
    Completed,
    /// A command failed or setup could not start.
    Failed,
}

/// Current setup snapshot retained in memory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetupSnapshot {
    /// Overall lifecycle.
    pub lifecycle: SetupLifecycle,
    /// Backing worktree or directory.
    pub worktree_path: String,
    /// Git branch when known.
    pub branch_name: String,
    /// Rendered bounded setup transcript.
    pub log: String,
    /// Per-command snapshots.
    pub commands: Vec<SetupCommandSnapshot>,
    /// Whether output was truncated.
    pub truncated: bool,
    /// Safe error text.
    pub error: Option<String>,
}

/// Workspace placement supplied to the runtime.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspacePlacement {
    /// Durable workspace identity.
    pub workspace_id: String,
    /// Directory containing `paseo.json` and used as command cwd.
    pub cwd: String,
    /// Backing worktree root.
    pub worktree_path: String,
    /// Main checkout root.
    pub repo_root: String,
    /// Current branch.
    pub branch_name: String,
}

/// Stable workspace automation failures.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum WorkspaceAutomationError {
    /// `paseo.json` is malformed, unsafe, or too large.
    #[error("{0}")]
    InvalidConfig(String),
    /// The named script is absent or malformed.
    #[error("Script '{0}' is not configured in paseo.json")]
    UnknownScript(String),
    /// A process with the same workspace/script key is already running.
    #[error("Script '{0}' is already running")]
    AlreadyRunning(String),
    /// No running process can be stopped.
    #[error("Script '{0}' is not running")]
    NotRunning(String),
    /// Process or filesystem work failed.
    #[error("{0}")]
    Io(String),
}

/// Runtime for background setup and configured scripts.
pub trait WorkspaceAutomationRuntime: Debug + Send + Sync {
    /// List configured scripts and refresh child exit state.
    ///
    /// # Errors
    /// Returns configuration, filesystem, or process inspection failures.
    fn list_scripts(
        &self,
        workspace: &WorkspacePlacement,
    ) -> Result<Vec<ScriptSnapshot>, WorkspaceAutomationError>;

    /// Start one configured script.
    ///
    /// # Errors
    /// Returns configuration, duplicate-run, or spawn failures.
    fn start_script(
        &self,
        workspace: &WorkspacePlacement,
        script_name: &str,
    ) -> Result<ScriptSnapshot, WorkspaceAutomationError>;

    /// Stop one running script.
    ///
    /// # Errors
    /// Returns missing-run or process termination failures.
    fn stop_script(
        &self,
        workspace: &WorkspacePlacement,
        script_name: &str,
    ) -> Result<ScriptSnapshot, WorkspaceAutomationError>;

    /// Start setup commands in the background. Returns false when a run is already active.
    ///
    /// # Errors
    /// Returns configuration or thread/process startup failures.
    fn start_setup(&self, workspace: &WorkspacePlacement)
    -> Result<bool, WorkspaceAutomationError>;

    /// Read the latest in-memory setup snapshot.
    fn setup_snapshot(&self, workspace_id: &str) -> Option<SetupSnapshot>;
}
